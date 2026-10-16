package dev.gpui.mobile;

import android.media.AudioFormat;
import android.media.AudioRecord;
import android.media.MediaRecorder;
import java.io.IOException;
import java.io.RandomAccessFile;

/** PCM16 recording; the writer owns file I/O until stop joins it. */
final class GpuiWavRecorder {
    private final AudioRecord recorder;
    private final RandomAccessFile file;
    private final int sampleRate, channels;
    private final Object writeLock = new Object();
    private final Thread writer;
    private final java.util.concurrent.CountDownLatch finished = new java.util.concurrent.CountDownLatch(1);
    private volatile boolean running = true;
    private volatile boolean paused;
    private volatile long generation;
    private volatile IOException failure;
    private volatile double amplitude;
    private long bytes;

    GpuiWavRecorder(String path, int rate, int channelCount) throws IOException {
        sampleRate = rate;
        channels = channelCount;
        int mask = channels == 1 ? AudioFormat.CHANNEL_IN_MONO : AudioFormat.CHANNEL_IN_STEREO;
        int minimum = AudioRecord.getMinBufferSize(rate, mask, AudioFormat.ENCODING_PCM_16BIT);
        if (minimum <= 0) throw new IOException("Unsupported PCM recording configuration");
        int capacity = Math.max(minimum, 4096);
        recorder = new AudioRecord(MediaRecorder.AudioSource.MIC, rate, mask,
            AudioFormat.ENCODING_PCM_16BIT, capacity);
        RandomAccessFile opened = null;
        try {
            if (recorder.getState() != AudioRecord.STATE_INITIALIZED) throw new IOException("AudioRecord initialization failed");
            opened = new RandomAccessFile(path, "rw");
            opened.setLength(44);
            opened.seek(44);
            recorder.startRecording();
            if (recorder.getRecordingState() != AudioRecord.RECORDSTATE_RECORDING) throw new IOException("Microphone did not start");
        } catch (Exception error) {
            recorder.release();
            if (opened != null) opened.close();
            throw new IOException("Cannot start WAV recorder", error);
        }
        file = opened;
        writer = new Thread(() -> capture(capacity / 2), "gpui-wav-recorder");
        writer.start();
    }

    private void capture(int capacity) {
        short[] samples = new short[capacity];
        byte[] encoded = new byte[capacity * 2];
        try {
            while (running) {
                long beforeRead = generation;
                int count = recorder.read(samples, 0, samples.length, AudioRecord.READ_NON_BLOCKING);
                if (!running) break;
                if (count < 0) throw new IOException("AudioRecord read failed: " + count);
                if (count == 0) {
                    try { Thread.sleep(10); }
                    catch (InterruptedException error) { Thread.currentThread().interrupt(); running = false; }
                    continue;
                }
                int peak = 0;
                for (int i = 0; i < count; i++) {
                    int value = samples[i];
                    peak = Math.max(peak, Math.abs(value));
                    encoded[2 * i] = (byte)value;
                    encoded[2 * i + 1] = (byte)(value >> 8);
                }
                synchronized (writeLock) {
                    if (!paused && beforeRead == generation) {
                        if (bytes + 2L * count > 0xffffffffL - 36) throw new IOException("WAV exceeds RIFF size limit");
                        file.write(encoded, 0, count * 2);
                        bytes += count * 2L;
                        amplitude = Math.min(1.0, peak / 32768.0);
                    }
                }
            }
        } catch (IOException error) { failure = error; }
        catch (RuntimeException error) { failure = new IOException("PCM capture failed", error); }
        finally {
            running = false;
            amplitude = 0;
            // The writer alone owns finalization, including terminal read/write
            // failure. A timed-out caller never closes resources under this thread.
            try { recorder.stop(); }
            catch (RuntimeException error) { if (failure == null) failure = new IOException("PCM stop failed", error); }
            try { file.seek(0); file.write(header(bytes, sampleRate, channels)); }
            catch (IOException error) { if (failure == null) failure = error; }
            finally {
                try { recorder.release(); }
                catch (RuntimeException error) { if (failure == null) failure = new IOException("PCM release failed", error); }
                try { file.close(); }
                catch (IOException error) { if (failure == null) failure = error; }
                finished.countDown();
            }
        }
    }

    private void requireActive() {
        if (!running || failure != null) throw new IllegalStateException("PCM recorder ended", failure);
    }
    boolean isRecording() { return running && failure == null && !paused; }
    void pause() { synchronized (writeLock) { requireActive(); generation++; paused = true; amplitude = 0; } }
    void resume() { synchronized (writeLock) { requireActive(); generation++; paused = false; } }
    double amplitude() { requireActive(); return paused ? 0 : amplitude; }

    long stop() throws IOException {
        running = false;
        try {
            if (!finished.await(2, java.util.concurrent.TimeUnit.SECONDS)) {
                throw new IOException("PCM finalization pending on its owning writer thread");
            }
        } catch (InterruptedException error) {
            Thread.currentThread().interrupt();
            throw new IOException("PCM finalization wait interrupted", error);
        }
        if (failure != null) throw failure;
        return bytes * 1000 / (sampleRate * (long)channels * 2);
    }

    static byte[] header(long bytes, int rate, int channels) throws IOException {
        if (bytes < 0 || bytes > 0xffffffffL - 36 || rate <= 0 || (channels != 1 && channels != 2)
                || rate * (long)channels * 2 > 0xffffffffL) throw new IOException("Invalid WAV sizes");
        java.nio.ByteBuffer h = java.nio.ByteBuffer.allocate(44).order(java.nio.ByteOrder.LITTLE_ENDIAN);
        h.put(new byte[]{'R','I','F','F'}).putInt((int)(bytes + 36));
        h.put(new byte[]{'W','A','V','E','f','m','t',' '}).putInt(16);
        h.putShort((short)1).putShort((short)channels).putInt(rate);
        h.putInt(rate * channels * 2).putShort((short)(channels * 2)).putShort((short)16);
        h.put(new byte[]{'d','a','t','a'}).putInt((int)bytes);
        return h.array();
    }
}

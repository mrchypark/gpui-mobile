package dev.gpui.mobile;

import android.media.AudioRecord;
import java.io.IOException;
import java.io.RandomAccessFile;
import java.nio.file.Files;
import java.nio.file.Path;

/** Uses production GpuiWavRecorder with only AudioRecord replaced by a fixture. */
public final class WavTerminalTest {
    public static void main(String[] args) throws Exception {
        for (int mode = 0; mode < 3; mode++) run(mode);
        System.out.println("WavTerminalTest PASS read/write/stop failure ownership");
    }
    private static void run(int mode) throws Exception {
        AudioRecord.released = AudioRecord.stopped = 0;
        AudioRecord.failRead = mode == 0;
        AudioRecord.failStop = mode == 2;
        AudioRecord.readGate = new java.util.concurrent.CountDownLatch(1);
        Path path = Files.createTempFile("gpui-wav-terminal-", ".wav");
        try {
            GpuiWavRecorder recorder = new GpuiWavRecorder(path.toString(), 8000, 1);
            if (mode == 1) {
                java.lang.reflect.Field field = GpuiWavRecorder.class.getDeclaredField("file");
                field.setAccessible(true);
                ((RandomAccessFile)field.get(recorder)).close(); // deterministic write failure
            }
            AudioRecord.readGate.countDown();
            if (mode != 2) {
                long deadline = System.nanoTime() + 2_000_000_000L;
                while (recorder.isRecording() && System.nanoTime() < deadline) Thread.yield();
                if (recorder.isRecording()) throw new AssertionError("terminal failure not published");
            }
            try { recorder.stop(); throw new AssertionError("terminal error swallowed"); }
            catch (IOException expected) {}
            if (AudioRecord.released != 1 || AudioRecord.stopped != 1) {
                throw new AssertionError("native resources not finalized exactly once");
            }
            try { recorder.resume(); throw new AssertionError("terminal recorder resumed"); }
            catch (IllegalStateException expected) {}
            try { recorder.pause(); throw new AssertionError("terminal recorder paused"); }
            catch (IllegalStateException expected) {}
        } finally { AudioRecord.readGate.countDown(); Files.deleteIfExists(path); }
    }
}

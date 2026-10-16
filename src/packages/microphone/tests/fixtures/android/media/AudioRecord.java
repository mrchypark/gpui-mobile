package android.media;

/** Runtime fixture for production PCM writer failure ownership tests. */
public final class AudioRecord {
    public static final int STATE_INITIALIZED = 1, RECORDSTATE_RECORDING = 3, READ_NON_BLOCKING = 1;
    public static volatile boolean failRead, failStop;
    public static volatile int released, stopped;
    public static volatile java.util.concurrent.CountDownLatch readGate;
    public AudioRecord(int source, int rate, int channels, int format, int capacity) {}
    public static int getMinBufferSize(int rate, int channels, int format) { return 4096; }
    public int getState() { return STATE_INITIALIZED; }
    public int getRecordingState() { return RECORDSTATE_RECORDING; }
    public void startRecording() {}
    public int read(short[] samples, int offset, int size, int mode) {
        try { if (readGate != null) readGate.await(); }
        catch (InterruptedException e) { Thread.currentThread().interrupt(); return -3; }
        if (failRead) return -3;
        samples[offset] = 100;
        return 1;
    }
    public void stop() { stopped++; if (failStop) throw new IllegalStateException("injected stop failure"); }
    public void release() { released++; }
}

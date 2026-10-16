package dev.gpui.mobile;
import java.nio.*;
import java.nio.charset.StandardCharsets;
public final class WavHeaderTest {
    public static void main(String[] args) throws Exception {
        byte[] bytes = GpuiWavRecorder.header(176400, 44100, 2);
        ByteBuffer h = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN);
        assert bytes.length == 44;
        assert new String(bytes, 0, 4, StandardCharsets.US_ASCII).equals("RIFF");
        assert new String(bytes, 8, 8, StandardCharsets.US_ASCII).equals("WAVEfmt ");
        assert h.getInt(4) == 176436 && h.getInt(40) == 176400;
        assert h.getShort(20) == 1 && h.getShort(22) == 2;
        assert h.getInt(24) == 44100 && h.getInt(28) == 176400;
        assert h.getShort(32) == 4 && h.getShort(34) == 16;
        assert ByteBuffer.wrap(GpuiWavRecorder.header(0, 8000, 1)).order(ByteOrder.LITTLE_ENDIAN).getInt(40) == 0;
        try { GpuiWavRecorder.header(0xffffffffL, 44100, 1); throw new AssertionError(); }
        catch (java.io.IOException expected) {}
        try { GpuiWavRecorder.header(1, 0, 3); throw new AssertionError(); }
        catch (java.io.IOException expected) {}
        System.out.println("PASS production PCM WAV header, stereo/mono/empty and overflow rejection");
    }
}

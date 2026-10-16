package dev.gpui.mobile;

/** Plain JVM boundary test; native SensorManager is never mocked or replaced. */
public final class GpuiSensorsTest {
    public static void main(String[] args) {
        float[] nativeAxes = { 0f, 9.80665f, -2f };
        double[] sample = GpuiSensors.reading(1, nativeAxes);
        check(sample != null && sample[0] == 0 && sample[1] == (double) nativeAxes[1] && sample[2] == -2);
        nativeAxes[1] = 0;
        check(sample[1] != 0); // SensorEvent arrays are reused by Android.
        check(GpuiSensors.reading(4, new float[] {1, 2, 3})[2] == 3);
        check(GpuiSensors.reading(2, new float[] {40, 0, -10})[0] == 40);
        check(GpuiSensors.reading(6, new float[] {1013.25f})[0] == 1013.25);
        check(GpuiSensors.reading(6, new float[] {0}) == null);
        check(GpuiSensors.reading(6, new float[] {-1}) == null);
        check(GpuiSensors.reading(1, new float[] {1, 2}) == null);
        check(GpuiSensors.reading(1, new float[] {1, Float.NaN, 3}) == null);
        check(GpuiSensors.reading(6, new float[] {Float.POSITIVE_INFINITY}) == null);
        check(GpuiSensors.reading(1, null) == null);
    }
    private static void check(boolean value) { if (!value) throw new AssertionError(); }
}

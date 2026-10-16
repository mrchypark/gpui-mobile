package dev.gpui.mobile;

/** Host-only input-domain regression. Does not simulate native listener/wake success. */
public final class GpuiThermalStatusTest {
    public static void main(String[] args) {
        for (int status = 0; status <= 6; status++)
            if (!GpuiThermal.validStatus(status)) throw new AssertionError("Dropped valid native state");
        for (int status : new int[]{-1, Integer.MIN_VALUE, 7, Integer.MAX_VALUE})
            if (GpuiThermal.validStatus(status)) throw new AssertionError("Accepted unknown/invalid state");
    }
}

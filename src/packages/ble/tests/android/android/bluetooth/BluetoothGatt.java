package android.bluetooth;
import java.util.*;
/** Recording native boundary only. The tested Peripheral command queue is production code. */
public class BluetoothGatt {
    public final List<BluetoothGattService> services = new ArrayList<>();
    public BluetoothGattCharacteristic read, write, notification;
    public BluetoothGattDescriptor descriptor;
    public boolean enabled;
    public int closeCount;
    public int disconnectCount;
    public void disconnect() { disconnectCount++; }
    public void close() { closeCount++; }
    public List<BluetoothGattService> getServices() { return services; }
    public BluetoothGattService getService(UUID id) { return services.stream().filter(s -> s.getUuid().equals(id)).findFirst().orElse(null); }
    public boolean readCharacteristic(BluetoothGattCharacteristic c) { read = c; return true; }
    public boolean writeCharacteristic(BluetoothGattCharacteristic c) { write = c; return true; }
    public boolean setCharacteristicNotification(BluetoothGattCharacteristic c, boolean enabled) { notification = c; this.enabled = enabled; return true; }
    public boolean writeDescriptor(BluetoothGattDescriptor d) { descriptor = d; return true; }
    public boolean readDescriptor(BluetoothGattDescriptor d) { descriptor = d; return true; }
}

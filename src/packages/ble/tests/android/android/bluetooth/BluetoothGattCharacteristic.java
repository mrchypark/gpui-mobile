package android.bluetooth;
import java.util.*;
public class BluetoothGattCharacteristic {
    private final UUID uuid;
    private final int properties, permissions;
    private byte[] value;
    public int writeType;
    BluetoothGattService service;
    private final List<BluetoothGattDescriptor> descriptors = new ArrayList<>();
    public BluetoothGattCharacteristic(UUID uuid, int properties, int permissions) { this.uuid = uuid; this.properties = properties; this.permissions = permissions; }
    public UUID getUuid() { return uuid; }
    public int getProperties() { return properties; }
    public int getPermissions() { return permissions; }
    public BluetoothGattService getService() { return service; }
    public boolean setValue(byte[] value) { this.value = value; return true; }
    public byte[] getValue() { return value; }
    public void setWriteType(int kind) { writeType = kind; }
    public boolean addDescriptor(BluetoothGattDescriptor d) { d.characteristic = this; return descriptors.add(d); }
    public List<BluetoothGattDescriptor> getDescriptors() { return descriptors; }
    public BluetoothGattDescriptor getDescriptor(UUID id) { return descriptors.stream().filter(d -> d.getUuid().equals(id)).findFirst().orElse(null); }
}

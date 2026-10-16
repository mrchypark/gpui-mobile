package android.bluetooth;
import java.util.UUID;
public class BluetoothGattDescriptor {
    public static final byte[] DISABLE_NOTIFICATION_VALUE = {0, 0};
    public static final byte[] ENABLE_NOTIFICATION_VALUE = {1, 0};
    public static final byte[] ENABLE_INDICATION_VALUE = {2, 0};
    private final UUID uuid;
    private byte[] value;
    BluetoothGattCharacteristic characteristic;
    public BluetoothGattDescriptor(UUID uuid, int permissions) { this.uuid = uuid; }
    public UUID getUuid() { return uuid; }
    public BluetoothGattCharacteristic getCharacteristic() { return characteristic; }
    public boolean setValue(byte[] value) { this.value = value; return true; }
    public byte[] getValue() { return value; }
}

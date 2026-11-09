package android.bluetooth;
import java.util.*;
public class BluetoothGattService {
    private final UUID uuid;
    private final int type;
    private final List<BluetoothGattCharacteristic> characteristics = new ArrayList<>();
    public BluetoothGattService(UUID uuid, int type) { this.uuid = uuid; this.type = type; }
    public UUID getUuid() { return uuid; }
    public int getType() { return type; }
    public List<BluetoothGattCharacteristic> getCharacteristics() { return characteristics; }
    public boolean addCharacteristic(BluetoothGattCharacteristic c) { c.service = this; return characteristics.add(c); }
    public BluetoothGattCharacteristic getCharacteristic(UUID id) { return characteristics.stream().filter(c -> c.getUuid().equals(id)).findFirst().orElse(null); }
}

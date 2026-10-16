package android.bluetooth;
public class BluetoothDevice {
    public static BluetoothGatt nextGatt;
    public BluetoothGatt connectGatt(android.content.Context context, boolean autoConnect, BluetoothGattCallback callback) {
        BluetoothGatt result = nextGatt;
        nextGatt = null;
        return result;
    }
    public String getAddress() { return "00:11:22:33:44:55"; }
}

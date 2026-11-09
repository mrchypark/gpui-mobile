package android.bluetooth;
public class BluetoothAdapter {
    public static BluetoothAdapter getDefaultAdapter() { return new BluetoothAdapter(); }
    public BluetoothDevice getRemoteDevice(String address) { return new BluetoothDevice(); }
}

package android.bluetooth;
public abstract class BluetoothGattCallback {
    public void onConnectionStateChange(BluetoothGatt gatt, int status, int newState) {}
    public void onServicesDiscovered(BluetoothGatt gatt, int status) {}
    public void onCharacteristicRead(BluetoothGatt g, BluetoothGattCharacteristic c, int status) {}
    public void onCharacteristicWrite(BluetoothGatt g, BluetoothGattCharacteristic c, int status) {}
    public void onDescriptorWrite(BluetoothGatt g, BluetoothGattDescriptor d, int status) {}
    public void onDescriptorRead(BluetoothGatt g, BluetoothGattDescriptor d, int status) {}
    public void onCharacteristicChanged(BluetoothGatt g, BluetoothGattCharacteristic c) {}
}

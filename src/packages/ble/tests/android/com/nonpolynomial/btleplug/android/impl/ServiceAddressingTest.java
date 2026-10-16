package com.nonpolynomial.btleplug.android.impl;
import android.bluetooth.*;
import java.util.*;
import java.lang.reflect.Field;
import io.github.gedgygedgy.rust.task.PollResult;

/** Executes real patched Peripheral methods with recording Android boundary objects; no radio claim. */
public class ServiceAddressingTest {
    static Object get(Object value, String name) throws Exception {
        Field field = value.getClass().getDeclaredField(name); field.setAccessible(true); return field.get(value);
    }
    static void set(Object value, String name, Object item) throws Exception {
        Field field = value.getClass().getDeclaredField(name); field.setAccessible(true); field.set(value, item);
    }
    static Object result(Object future) throws Exception { return ((PollResult<?>)get(future, "result")).get(); }
    static void check(boolean condition) { if (!condition) throw new AssertionError(); }
    public static void main(String[] args) throws Exception {
        UUID a = UUID.fromString("0000180d-0000-1000-8000-00805f9b34fb");
        UUID b = UUID.fromString("0000180f-0000-1000-8000-00805f9b34fb");
        UUID id = UUID.fromString("00002a37-0000-1000-8000-00805f9b34fb");
        UUID cccd = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb");
        BluetoothGatt gatt = new BluetoothGatt();
        BluetoothGattService sa = new BluetoothGattService(a, 0), sb = new BluetoothGattService(b, 0);
        BluetoothGattCharacteristic ca = new BluetoothGattCharacteristic(id, 0x12, 0), cb = new BluetoothGattCharacteristic(id, 0x22, 0);
        sa.addCharacteristic(ca); sb.addCharacteristic(cb); gatt.services.add(sa); gatt.services.add(sb);
        BluetoothGattDescriptor da = new BluetoothGattDescriptor(cccd, 0), db = new BluetoothGattDescriptor(cccd, 0);
        ca.addDescriptor(da); cb.addDescriptor(db);
        ca.setValue(new byte[]{1}); cb.setValue(new byte[]{2});
        List<Boolean> connectionEvents = new ArrayList<>();
        Set<String> observedCache = new HashSet<>();
        String address = "00:11:22:33:44:55";
        observedCache.add(address);
        Peripheral p = new Peripheral(new Adapter() {
            @Override public void onConnectionStateChanged(String deviceAddress, boolean connected) {
                connectionEvents.add(connected);
                if (connected) observedCache.add(deviceAddress);
                else observedCache.remove(deviceAddress);
            }
        }, "00:11:22:33:44:55");
        set(p, "gatt", gatt); set(p, "connected", true);
        BluetoothGattCallback callback = (BluetoothGattCallback)get(p, "callback");
        Object read = p.read(a, id);
        Object write = p.write(b, id, new byte[]{9}, 2); // queued, must retain its own service
        check(gatt.read == ca && gatt.write == null);
        callback.onCharacteristicRead(gatt, ca, 0);
        check(Arrays.equals((byte[])result(read), new byte[]{1}));
        check(gatt.write == cb && cb.getValue()[0] == 9 && ca.getValue()[0] == 1);
        callback.onCharacteristicWrite(gatt, cb, 0); result(write);
        Object notify = p.setCharacteristicNotification(a, id, true);
        check(gatt.notification == ca && gatt.enabled && gatt.descriptor == da && da.getValue()[0] == 1);
        callback.onDescriptorWrite(gatt, da, 0); result(notify);
        Object disable = p.setCharacteristicNotification(b, id, false);
        check(gatt.notification == cb && !gatt.enabled && gatt.descriptor == db && db.getValue()[0] == 0);
        callback.onDescriptorWrite(gatt, db, 0); result(disable);
        Object indicate = p.setCharacteristicNotification(b, id, true);
        check(gatt.notification == cb && db.getValue()[0] == 2);
        callback.onDescriptorWrite(gatt, db, 0); result(indicate);
        Object wrong = p.read(b, id);
        callback.onCharacteristicRead(gatt, ca, 0);
        try { result(wrong); throw new AssertionError("wrong service callback accepted"); }
        catch (io.github.gedgygedgy.rust.future.FutureException expected) {
            check(expected.getCause() instanceof UnexpectedCharacteristicException);
        }
        Object stream = p.getNotifications();
        callback.onCharacteristicChanged(gatt, cb);
        @SuppressWarnings("unchecked") Queue<BluetoothGattCharacteristic> values = (Queue<BluetoothGattCharacteristic>)get(stream, "result");
        BluetoothGattCharacteristic snapshot = values.remove();
        check(snapshot != cb && snapshot.getService().getUuid().equals(b) && snapshot.getValue()[0] == 9);
        Object descriptor = p.readDescriptor(b, id, cccd);
        check(gatt.descriptor == db);
        callback.onDescriptorRead(gatt, db, 0); result(descriptor);
        Object descriptorWrite = p.writeDescriptor(a, id, cccd, new byte[]{7});
        check(gatt.descriptor == da && da.getValue()[0] == 7 && db.getValue()[0] == 2);
        callback.onDescriptorWrite(gatt, da, 0); result(descriptorWrite);
        callback.onConnectionStateChange(gatt, 0, 0); // idle remote disconnect
        check(gatt.closeCount == 1 && get(p, "gatt") == null && !p.isConnected());
        result(p.disconnect());
        check(gatt.closeCount == 1); // explicit close after remote close is idempotent
        check(connectionEvents.equals(Arrays.asList(false)) && !observedCache.contains(address));
        BluetoothGatt replacement = new BluetoothGatt();
        set(p, "gatt", replacement); set(p, "connected", true);
        callback.onConnectionStateChange(gatt, 0, 0); // stale old client callback
        check(get(p, "gatt") == replacement && replacement.closeCount == 0 && p.isConnected());
        callback.onConnectionStateChange(replacement, 0, 0);
        check(replacement.closeCount == 1 && get(p, "gatt") == null);
        BluetoothGatt alreadyDisconnected = new BluetoothGatt();
        set(p, "gatt", alreadyDisconnected);
        result(p.disconnect()); result(p.disconnect());
        check(alreadyDisconnected.closeCount == 1 && get(p, "gatt") == null);
        check(connectionEvents.size() == 3); // idle replacement + residual client, not repeated close
        BluetoothGatt stalled = new BluetoothGatt();
        stalled.services.add(sa);
        set(p, "gatt", stalled); set(p, "connected", true);
        Object abandonedRead = p.read(a, id); // deliberately never deliver callback
        Object queuedWrite = p.write(a, id, new byte[]{3}, 2);
        check(stalled.read == ca && stalled.write == null);
        result(p.disconnect()); // must retire immediately, without read callback
        check(stalled.disconnectCount == 1 && stalled.closeCount == 1);
        check(connectionEvents.size() == 4 && !observedCache.contains(address));
        interrupted(abandonedRead); interrupted(queuedWrite);
        BluetoothGatt fresh = new BluetoothGatt(); fresh.services.add(sa);
        BluetoothDevice.nextGatt = fresh;
        Object connecting = p.connect(); // actual production connect path, new client
        callback.onConnectionStateChange(stalled, 0, 2);
        check(get(connecting, "result") == null);
        callback.onConnectionStateChange(fresh, 0, 2); result(connecting);
        check(connectionEvents.equals(Arrays.asList(false, false, false, false, true)));
        check(observedCache.contains(address));
        Object freshRead = p.read(a, id);
        callback.onCharacteristicRead(stalled, ca, 0);
        callback.onServicesDiscovered(stalled, 0);
        callback.onConnectionStateChange(stalled, 0, 0);
        check(get(freshRead, "result") == null && fresh.closeCount == 0 && p.isConnected());
        check(connectionEvents.size() == 5 && observedCache.contains(address));
        callback.onCharacteristicRead(fresh, ca, 0); result(freshRead);
        interrupted(abandonedRead); // late callback did not replace original interruption
        result(p.disconnect());
        check(fresh.closeCount == 1);
        check(connectionEvents.size() == 6 && !connectionEvents.get(5) && !observedCache.contains(address));
        result(p.disconnect());
        callback.onConnectionStateChange(fresh, 0, 0);
        check(connectionEvents.size() == 6); // ordinary close plus late callback emits exactly once
        System.out.println("PASS service addressing, idle cleanup, stalled FIFO abort and fresh-client recovery");
    }
    static void interrupted(Object future) throws Exception {
        try { result(future); throw new AssertionError("pending operation not interrupted"); }
        catch (io.github.gedgygedgy.rust.future.FutureException expected) {
            check(expected.getCause() instanceof NotConnectedException);
        }
    }
}

import Foundation
import CoreBluetooth
final class Echo: NSObject, CBPeripheralManagerDelegate {
    let serviceID = CBUUID(string: "928D3811-55B1-4AF2-A9E7-1B0C6AC20488")
    let characteristicID = CBUUID(string: "928D3812-55B1-4AF2-A9E7-1B0C6AC20488")
    var manager: CBPeripheralManager!
    var characteristic: CBMutableCharacteristic!
    var current = Data("initial".utf8)
    var subscribers = Set<UUID>()
    override init() { super.init(); manager = CBPeripheralManager(delegate: self, queue: nil) }
    func peripheralManagerDidUpdateState(_ peripheral: CBPeripheralManager) {
        print("STATE \(peripheral.state.rawValue)"); fflush(stdout)
        guard peripheral.state == .poweredOn else { return }
        characteristic = CBMutableCharacteristic(type: characteristicID, properties: [.read, .write, .writeWithoutResponse, .notify], value: nil, permissions: [.readable, .writeable])
        let service = CBMutableService(type: serviceID, primary: true)
        service.characteristics = [characteristic]
        peripheral.add(service)
    }
    func peripheralManager(_ peripheral: CBPeripheralManager, didAdd service: CBService, error: Error?) {
        if let error { print("ADD_FAIL \(error.localizedDescription)") } else {
            peripheral.startAdvertising([CBAdvertisementDataServiceUUIDsKey: [serviceID], CBAdvertisementDataLocalNameKey: "GPUI-GATT-TEST"])
        }; fflush(stdout)
    }
    func peripheralManagerDidStartAdvertising(_ peripheral: CBPeripheralManager, error: Error?) {
        print(error == nil ? "ADVERTISING" : "ADVERTISE_FAIL"); fflush(stdout)
    }
    func peripheralManager(_ peripheral: CBPeripheralManager, didReceiveRead request: CBATTRequest) {
        guard request.characteristic.uuid == characteristicID, request.offset <= current.count else { peripheral.respond(to: request, withResult: .invalidOffset); return }
        request.value = current.subdata(in: request.offset..<current.count)
        peripheral.respond(to: request, withResult: .success); print("READ_OK"); fflush(stdout)
    }
    func peripheralManager(_ peripheral: CBPeripheralManager, didReceiveWrite requests: [CBATTRequest]) {
        guard let request = requests.first, request.characteristic.uuid == characteristicID, request.offset == 0, let value = request.value else {
            if let request = requests.first { peripheral.respond(to: request, withResult: .invalidOffset) }; return
        }
        current = value; peripheral.respond(to: request, withResult: .success)
        let notified = peripheral.updateValue(current, for: characteristic, onSubscribedCentrals: nil)
        print("WRITE_OK notification_queued=\(notified)"); fflush(stdout)
    }
    func peripheralManager(_ peripheral: CBPeripheralManager, central: CBCentral, didSubscribeTo characteristic: CBCharacteristic) { subscribers.insert(central.identifier); print("SUBSCRIBED count=\(subscribers.count)"); fflush(stdout) }
    func peripheralManager(_ peripheral: CBPeripheralManager, central: CBCentral, didUnsubscribeFrom characteristic: CBCharacteristic) { subscribers.remove(central.identifier); print("UNSUBSCRIBED count=\(subscribers.count)"); fflush(stdout) }
}
let echo = Echo()
Timer.scheduledTimer(withTimeInterval: 0.2, repeats: true) { _ in
    let trigger = "/tmp/gpui-gatt-fixture/background-trigger"
    guard FileManager.default.fileExists(atPath: trigger), echo.characteristic != nil else { return }
    let sequence = (try? String(contentsOfFile: trigger, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)) ?? ""
    guard sequence.isEmpty || (sequence.count <= 6 && sequence.allSatisfy({ $0.isASCII && $0.isNumber })) else { return }
    if echo.subscribers.isEmpty {
        try? FileManager.default.removeItem(atPath: trigger)
        print("BACKGROUND_NO_SUBSCRIBERS"); fflush(stdout)
        return
    }
    let value = Data((sequence.isEmpty ? "gpui-background-push" : "gpui-survival:\(sequence)").utf8)
    if echo.manager.updateValue(value, for: echo.characteristic, onSubscribedCentrals: nil) {
        try? FileManager.default.removeItem(atPath: trigger)
        print("BACKGROUND_NOTIFICATION_QUEUED sequence=\(sequence) subscribers=\(echo.subscribers.count)"); fflush(stdout)
    }
}
RunLoop.main.run()

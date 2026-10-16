package android.bluetooth.le;
import java.util.List;
/** Constructor boundary for Adapter; this test never starts a native scan. */
public abstract class ScanCallback {
    public void onScanFailed(int errorCode) {}
    public void onBatchScanResults(List<ScanResult> results) {}
    public void onScanResult(int callbackType, ScanResult result) {}
}

package dev.gpui.mobile;
import com.google.android.gms.location.*;
import com.google.android.gms.common.api.ResolvableApiException;
import android.content.Context;
import android.location.Location;
import java.util.*;
public final class GmsOwnershipTest implements GpuiGmsLocation.Listener {
    int ready,closed,cleanup; String reason; List<Long> fixes=new ArrayList<>(); GpuiGmsLocation owner; boolean stopOnFix;
    public void ready(){ready++;} public void location(Location l){fixes.add(l.getElapsedRealtimeNanos());if(stopOnFix)owner.stop();}
    public void closed(String s){closed++;reason=s;} public void cleanupFailed(){cleanup++;}
    static GmsOwnershipTest create(boolean fine){LocationServices.reset();GmsOwnershipTest t=new GmsOwnershipTest();t.owner=new GpuiGmsLocation(new Context(),t);t.owner.start(3,0,fine);return t;}
    static void check(boolean v){if(!v)throw new AssertionError();}
    public static void main(String[] args) throws Exception {
        if (!GpuiGmsLocation.settingsStates(null).equals(" states=unavailable")) throw new AssertionError("missing is not false");
        String settings = GpuiGmsLocation.settingsStates(new LocationSettingsStates());
        if (!settings.equals(" gps_present=true gps_usable=false network_present=true network_usable=false ble_present=true ble_usable=false location_present=true location_usable=false")) throw new AssertionError(settings);
        GmsOwnershipTest t=create(true); t.owner.stop();LocationServices.settings.task.finish(null);
        check(t.closed==1&&LocationServices.client.starts==0); // cancel pending settings
        t=create(true);LocationServices.settings.task.finish(null);t.owner.stop();
        check(LocationServices.client.removals.size()==1);LocationServices.client.removals.get(0).finish(null);
        check(t.closed==0);LocationServices.client.registration.finish(null);
        check(LocationServices.client.removals.size()==2&&t.ready==0);
        LocationServices.client.callback.onLocationResult(new LocationResult(1));check(t.fixes.isEmpty());
        LocationServices.client.removals.get(1).finish(null);check(t.closed==1);
        // Opposite completion order must also remove again after registration.
        t=create(true);LocationServices.settings.task.finish(null);t.owner.stop();LocationServices.client.registration.finish(null);
        LocationServices.client.removals.get(0).finish(null);check(LocationServices.client.removals.size()==2&&t.closed==0);
        LocationServices.client.removals.get(1).finish(null);check(t.closed==1&&t.ready==0);
        for (boolean canceled : new boolean[]{false,true}) {
            t=create(true);LocationServices.settings.task.finish(null);t.owner.stop();
            LocationServices.client.removals.get(0).finish(null);
            if(canceled)LocationServices.client.registration.cancel();else LocationServices.client.registration.finish(new RuntimeException());
            check(LocationServices.client.removals.size()==2&&t.closed==0&&t.ready==0);
            LocationServices.client.removals.get(1).finish(null);check(t.closed==1&&t.reason.equals("stopped"));
        }
        t=create(true);LocationServices.settings.task.finish(null);GpuiGmsLocation.retire(t.owner);
        LocationServices.client.removals.get(0).finish(new RuntimeException());
        check(GpuiGmsLocation.RETIRED.containsKey(t.owner));
        LocationServices.client.registration.finish(new RuntimeException());
        check(LocationServices.client.removals.size()==2&&GpuiGmsLocation.cleanupPending());
        LocationServices.client.callback.onLocationResult(new LocationResult(1));check(t.fixes.isEmpty());
        LocationServices.client.removals.get(1).finish(null);
        check(!GpuiGmsLocation.cleanupPending()&&t.closed==1);
        // A fresh one-shot callback can precede registration Task success.
        t=create(true);t.stopOnFix=true;LocationServices.settings.task.finish(null);
        LocationServices.client.callback.onLocationResult(new LocationResult(3,1,2));check(t.fixes.equals(Arrays.asList(1L)));
        LocationServices.client.registration.finish(null);LocationServices.client.removals.get(0).finish(null);
        LocationServices.client.removals.get(1).finish(null);check(t.ready==0&&t.closed==1);
        t=create(false);LocationRequest r=LocationServices.settings.request.request;
        check(r.priority==Priority.PRIORITY_BALANCED_POWER_ACCURACY&&r.interval==1000&&r.min==1000&&r.age==0&&r.delay==0&&!r.wait);
        LocationServices.settings.task.finish(null);LocationServices.client.registration.finish(null);
        LocationServices.client.callback.onLocationResult(new LocationResult(3,1,2));check(t.ready==1&&t.fixes.equals(Arrays.asList(1L,2L,3L)));
        t.owner.stop();LocationServices.client.removals.get(0).finish(new RuntimeException());
        check(t.cleanup==1&&t.closed==0);t.owner.stop();LocationServices.client.removals.get(1).finish(null);check(t.closed==1);
        t=create(true);LocationServices.settings.task.finish(new ResolvableApiException());
        check(t.reason.equals("settings_resolution_required")&&LocationServices.client.starts==0);
        t=create(true);LocationServices.settings.task.finish(new SecurityException());check(t.reason.equals("permission_denied"));
        t=create(true);LocationServices.settings.task.finish(new RuntimeException());check(t.reason.equals("settings_unavailable"));
        t=create(true);LocationServices.settings.task.cancel();check(t.reason.equals("settings_unavailable"));
        t=create(true);LocationServices.settings.task.finish(null);LocationServices.client.registration.cancel();
        check(t.closed==0);LocationServices.client.removals.get(0).finish(null);check(t.reason.equals("registration_failed"));
        t=create(true);LocationServices.settings.task.finish(null);LocationServices.client.registration.finish(new RuntimeException());
        check(t.closed==0&&t.ready==0);LocationServices.client.removals.get(0).finish(null);check(t.reason.equals("registration_failed"));
        t=create(true);LocationServices.client.thrown=new SecurityException();LocationServices.settings.task.finish(null);
        LocationServices.client.removals.get(0).finish(null);check(t.reason.equals("permission_denied"));
        com.google.android.gms.common.GoogleApiAvailability.status=1;check(!GpuiGmsLocation.available(new Context()));
        // Actual stopOnMain removes logical subscription but retains native retirement.
        t=create(true);LocationServices.settings.task.finish(null);LocationServices.client.registration.finish(null);
        Class<?> host=Class.forName("dev.gpui.mobile.GpuiLocation");
        Class<?> subscription=Class.forName("dev.gpui.mobile.GpuiLocation$Subscription");
        java.lang.reflect.Constructor<?> constructor=subscription.getDeclaredConstructors()[0];constructor.setAccessible(true);
        Object s=constructor.newInstance(new android.app.Activity(),77L,3,0.0,false,false);
        java.lang.reflect.Field gms=subscription.getDeclaredField("gms");gms.setAccessible(true);gms.set(s,t.owner);
        java.lang.reflect.Field registry=host.getDeclaredField("SUBSCRIPTIONS");registry.setAccessible(true);
        @SuppressWarnings("unchecked") Map<Long,Object> map=(Map<Long,Object>)registry.get(null);map.put(77L,s);
        java.lang.reflect.Method stop=host.getDeclaredMethod("stopOnMain",long.class);stop.setAccessible(true);stop.invoke(null,77L);
        check(!map.containsKey(77L)&&GpuiGmsLocation.RETIRED.containsKey(t.owner));
        LocationServices.client.removals.get(0).finish(new RuntimeException());
        List<Runnable> retries=new ArrayList<>(android.os.Handler.delayed);android.os.Handler.delayed.clear();
        for(Runnable retry:retries)retry.run();
        check(LocationServices.client.removals.size()==2); // production scheduled retry, no old-token refresh
        check(GpuiGmsLocation.cleanupPending()); // replacement remains blocked
        LocationServices.client.callback.onLocationResult(new LocationResult(1));check(t.fixes.isEmpty());
        LocationServices.client.removals.get(1).finish(null);
        check(!GpuiGmsLocation.cleanupPending()&&t.closed==1);
        System.out.println("PASS production GMS adapter: pending settings/registration cancel, both completion orders, early callback, ordered batch, coarse/settings/security, cleanup retry");
    }
}

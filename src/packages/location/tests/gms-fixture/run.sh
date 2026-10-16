#!/bin/sh
# Executes production GpuiGmsLocation against deterministic Task/SDK boundaries.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo=$(CDPATH= cd -- "$here/../../../../.." && pwd)
scratch=$(mktemp -d /private/tmp/gpui-gms-test.XXXXXX)
trap 'rm -rf "$scratch"; test ! -e "$scratch" && echo CLEANUP-fixture-removed' EXIT HUP INT TERM
cd "$scratch"
mkdir -p android/content/pm android/app android/location android/os com/google/android/gms/common/api com/google/android/gms/location dev/gpui/mobile
cat > android/content/pm/ApplicationInfo.java <<'JAVA'
package android.content.pm;public class ApplicationInfo {public static final int FLAG_DEBUGGABLE=2;public static int testFlags;public int flags=testFlags;}
JAVA
cat > android/app/Application.java <<'JAVA'
package android.app;public class Application extends android.content.Context {public interface ActivityLifecycleCallbacks {} public void unregisterActivityLifecycleCallbacks(ActivityLifecycleCallbacks c){}}
JAVA
cat > android/app/Activity.java <<'JAVA'
package android.app;public class Activity extends android.content.Context {public Application getApplication(){return new Application();}}
JAVA
cat > android/os/SystemClock.java <<'JAVA'
package android.os;public class SystemClock {public static long elapsedRealtimeNanos(){return 100;}public static long elapsedRealtime(){return 0;}}
JAVA
cat > android/content/Context.java <<'JAVA'
package android.content; public class Context { public static final String LOCATION_SERVICE="location"; public Object getSystemService(String s){return null;} public <T> T getSystemService(Class<T> type){return null;} public android.content.pm.ApplicationInfo getApplicationInfo(){return new android.content.pm.ApplicationInfo();} }
JAVA
cat > android/os/Handler.java <<'JAVA'
package android.os; public class Handler {public static java.util.List<Runnable> delayed=new java.util.ArrayList<>();public Handler(Looper l){}public boolean post(Runnable r){r.run();return true;}public boolean postDelayed(Runnable r,long d){delayed.add(r);return true;}public void removeCallbacks(Runnable r){delayed.remove(r);}}
JAVA
cat > android/os/Looper.java <<'JAVA'
package android.os; public class Looper {public static Looper getMainLooper(){return new Looper();}}
JAVA
cat > android/location/Location.java <<'JAVA'
package android.location; public class Location {final long time; public Location(long t){time=t;} public long getElapsedRealtimeNanos(){return time;} public double getLatitude(){return 1;}public double getLongitude(){return 2;}public boolean hasAccuracy(){return true;}public float getAccuracy(){return 3;}public long getTime(){return time;}public String getProvider(){return "test";}public boolean hasAltitude(){return false;}public double getAltitude(){return 0;}public boolean hasSpeed(){return false;}public float getSpeed(){return 0;}public boolean hasSpeedAccuracy(){return false;}public float getSpeedAccuracyMetersPerSecond(){return 0;}public boolean hasBearing(){return false;}public float getBearing(){return 0;}public boolean hasBearingAccuracy(){return false;}public float getBearingAccuracyDegrees(){return 0;}}
JAVA
cat > com/google/android/gms/common/ConnectionResult.java <<'JAVA'
package com.google.android.gms.common; public class ConnectionResult {public static final int SUCCESS=0;}
JAVA
cat > com/google/android/gms/common/GoogleApiAvailability.java <<'JAVA'
package com.google.android.gms.common; public class GoogleApiAvailability {public static int status; public static GoogleApiAvailability getInstance(){return new GoogleApiAvailability();} public int isGooglePlayServicesAvailable(android.content.Context c){return status;}}
JAVA
mkdir -p android/util
cat > android/util/Log.java <<'JAVA'
package android.util;public class Log {public static final java.util.List<String> messages=new java.util.ArrayList<>();public static int i(String t,String m){messages.add(m);return 0;}}
JAVA
cat > com/google/android/gms/common/api/ApiException.java <<'JAVA'
package com.google.android.gms.common.api;public class ApiException extends Exception {public int getStatusCode(){return 6;}}
JAVA
cat > com/google/android/gms/common/api/ResolvableApiException.java <<'JAVA'
package com.google.android.gms.common.api; public class ResolvableApiException extends ApiException {public Object getResolution(){return new Object();}}
JAVA
cat > com/google/android/gms/location/LocationSettingsStates.java <<'JAVA'
package com.google.android.gms.location;public class LocationSettingsStates {public boolean isGpsPresent(){return true;}public boolean isGpsUsable(){return false;}public boolean isNetworkLocationPresent(){return true;}public boolean isNetworkLocationUsable(){return false;}public boolean isBlePresent(){return true;}public boolean isBleUsable(){return false;}public boolean isLocationPresent(){return true;}public boolean isLocationUsable(){return false;}}
JAVA
cat > com/google/android/gms/location/LocationSettingsResponse.java <<'JAVA'
package com.google.android.gms.location;public class LocationSettingsResponse {public LocationSettingsStates getLocationSettingsStates(){return new LocationSettingsStates();}}
JAVA
cat > com/google/android/gms/location/Task.java <<'JAVA'
package com.google.android.gms.location;
public class Task { public Exception error; public boolean done,canceled;
 public LocationSettingsResponse getResult(){return new LocationSettingsResponse();}
 java.util.function.Consumer<Object> success; java.util.function.Consumer<Exception> failure; java.util.function.Consumer<Task> complete;
 public Task addOnSuccessListener(java.util.concurrent.Executor e,java.util.function.Consumer<Object> c){success=c;return this;}
 public Task addOnFailureListener(java.util.concurrent.Executor e,java.util.function.Consumer<Exception> c){failure=c;return this;}
 public Task addOnCompleteListener(java.util.concurrent.Executor e,java.util.function.Consumer<Task> c){complete=c;return this;}
 public boolean isSuccessful(){return done&&!canceled&&error==null;} public Exception getException(){return error;}
 public void cancel(){done=true;canceled=true;if(complete!=null)complete.accept(this);}
 public void finish(Exception e){done=true;error=e;if(e==null&&success!=null)success.accept(null);if(e!=null&&failure!=null)failure.accept(e);if(complete!=null)complete.accept(this);}
}
JAVA
cat > com/google/android/gms/location/LocationCallback.java <<'JAVA'
package com.google.android.gms.location; public class LocationCallback {public void onLocationResult(LocationResult r){}}
JAVA
cat > com/google/android/gms/location/LocationResult.java <<'JAVA'
package com.google.android.gms.location; public class LocationResult {final java.util.List<android.location.Location> values; public LocationResult(long... times){values=new java.util.ArrayList<>();for(long t:times)values.add(new android.location.Location(t));} public java.util.List<android.location.Location> getLocations(){return values;}}
JAVA
cat > com/google/android/gms/location/Priority.java <<'JAVA'
package com.google.android.gms.location; public class Priority {public static final int PRIORITY_HIGH_ACCURACY=100,PRIORITY_BALANCED_POWER_ACCURACY=102,PRIORITY_LOW_POWER=104;}
JAVA
cat > com/google/android/gms/location/LocationRequest.java <<'JAVA'
package com.google.android.gms.location;
public class LocationRequest {public int priority;public long interval,min,delay,age;public float distance;public boolean wait;
 public static class Builder {LocationRequest r=new LocationRequest();public Builder(int p,long i){r.priority=p;r.interval=i;}public Builder setMinUpdateIntervalMillis(long n){r.min=n;return this;}public Builder setMinUpdateDistanceMeters(float n){r.distance=n;return this;}public Builder setMaxUpdateDelayMillis(long n){r.delay=n;return this;}public Builder setMaxUpdateAgeMillis(long n){r.age=n;return this;}public Builder setWaitForAccurateLocation(boolean n){r.wait=n;return this;}public LocationRequest build(){return r;}}
}
JAVA
cat > com/google/android/gms/location/LocationSettingsRequest.java <<'JAVA'
package com.google.android.gms.location; public class LocationSettingsRequest {public LocationRequest request;public static class Builder {LocationSettingsRequest r=new LocationSettingsRequest();public Builder addLocationRequest(LocationRequest q){r.request=q;return this;}public LocationSettingsRequest build(){return r;}}}
JAVA
cat > com/google/android/gms/location/SettingsClient.java <<'JAVA'
package com.google.android.gms.location; public class SettingsClient {public Task task=new Task();public LocationSettingsRequest request;public Task checkLocationSettings(LocationSettingsRequest r){request=r;return task;}public Task isGoogleLocationAccuracyEnabled(){return new Task();}}
JAVA
cat > com/google/android/gms/location/FusedLocationProviderClient.java <<'JAVA'
package com.google.android.gms.location;
public class FusedLocationProviderClient {public Task registration=new Task();public java.util.List<Task> removals=new java.util.ArrayList<>();public LocationCallback callback;public int starts;public RuntimeException thrown;
public Task requestLocationUpdates(LocationRequest r,LocationCallback c,android.os.Looper l){starts++;callback=c;if(thrown!=null)throw thrown;return registration;}
public Task removeLocationUpdates(LocationCallback c){if(c!=callback)throw new AssertionError("wrong callback");Task t=new Task();removals.add(t);return t;}}
JAVA
cat > com/google/android/gms/location/LocationServices.java <<'JAVA'
package com.google.android.gms.location;public class LocationServices {public static FusedLocationProviderClient client;public static SettingsClient settings;public static void reset(){client=new FusedLocationProviderClient();settings=new SettingsClient();}public static FusedLocationProviderClient getFusedLocationProviderClient(android.content.Context c){return client;}public static SettingsClient getSettingsClient(android.content.Context c){return settings;}}
JAVA
cp "$here/GmsOwnershipTest.java" dev/gpui/mobile/
cp "$repo/example/android/gradle/app/src/main/java/dev/gpui/mobile/GpuiGmsLocation.java" dev/gpui/mobile/
find . -name '*.java' > sources
javac=${JAVA_HOME:+$JAVA_HOME/bin/}javac
java=${JAVA_HOME:+$JAVA_HOME/bin/}java
"$javac" -J-Xmx128m @sources
# Compile the actual subscription owner against real APIs, then run it with the
# controlled Android boundary above. No rewritten copy of stopOnMain is used.
: "${LOCATION_JAVA_CLASSPATH_FILE:?Set resolved real Android/GMS classpath file}"
classpath=$(cat "$LOCATION_JAVA_CLASSPATH_FILE")
mkdir sdk
source_dir="$repo/example/android/gradle/app/src/main/java/dev/gpui/mobile"
"$javac" -J-Xmx256m -cp "$classpath" -d sdk "$source_dir/GpuiLocation.java" "$source_dir/GpuiGmsLocation.java" "$source_dir/GpuiLocationService.java"
"$java" -ea -cp ".:sdk:$classpath" dev.gpui.mobile.GmsOwnershipTest

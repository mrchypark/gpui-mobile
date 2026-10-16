// Compile with ARC and link CoreLocation/Foundation. All managers live on main.
#import <CoreLocation/CoreLocation.h>
#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <float.h>

extern void gpui_location_event(uint64_t token, const char *event);
static NSMutableDictionary<NSNumber *, id> *subscriptions;
static NSString *lastPosition;
static NSDate *lastPositionTime;

static void configureManager(CLLocationManager *manager, double accuracy, double distance) {
    manager.desiredAccuracy = accuracy;
    manager.distanceFilter = distance == 0 ? kCLDistanceFilterNone : distance;
    // Foreground watches must not silently stop after an automatic OS pause.
    manager.pausesLocationUpdatesAutomatically = NO;
    manager.allowsBackgroundLocationUpdates = NO;
}

static BOOL validLocation(CLLocation *location) {
    return location && isfinite(location.horizontalAccuracy) && location.horizontalAccuracy >= 0
        && CLLocationCoordinate2DIsValid(location.coordinate)
        && isfinite(location.timestamp.timeIntervalSince1970) && location.timestamp.timeIntervalSince1970 >= 0;
}

static NSString *serialize(CLLocation *location) {
    return [NSString stringWithFormat:@"%.17g|%.17g|%.17g|%.17g|%.17g|%.17g|%.17g|%.17g|%llu",
        location.coordinate.latitude, location.coordinate.longitude,
        location.verticalAccuracy >= 0 ? location.altitude : NAN, location.horizontalAccuracy,
        location.speed >= 0 ? location.speed : NAN,
        location.speedAccuracy >= 0 ? location.speedAccuracy : NAN,
        location.course >= 0 ? location.course : NAN,
        location.courseAccuracy >= 0 ? location.courseAccuracy : NAN,
        (unsigned long long)(location.timestamp.timeIntervalSince1970 * 1000.0)];
}

@interface GpuiLocationSubscription : NSObject <CLLocationManagerDelegate>
@property(nonatomic) uint64_t token;
@property(nonatomic) BOOL once;
@property(nonatomic) BOOL active;
@property(nonatomic) BOOL suspended;
@property(nonatomic) BOOL backgroundTracking;
@property(nonatomic) BOOL updating;
@property(nonatomic) NSDate *requestedAt;
@property(nonatomic) CLLocationManager *manager;
- (void)emit:(NSString *)event;
- (void)stop;
- (void)finish:(NSString *)reason;
- (void)authorize;
@end
@implementation GpuiLocationSubscription
- (void)emit:(NSString *)event {
    if (self.active) gpui_location_event(self.token, event.UTF8String);
}
- (void)stop {
    self.active = NO;
    self.updating = NO;
    [self.manager stopUpdatingLocation];
    self.manager.delegate = nil;
    [[NSNotificationCenter defaultCenter] removeObserver:self];
    if (subscriptions[@(self.token)] == self) [subscriptions removeObjectForKey:@(self.token)];
}
- (void)finish:(NSString *)reason {
    [self emit:[@"end|" stringByAppendingString:reason]];
    [self stop];
}
- (void)authorize {
    if (!self.active || self.suspended) return;
    CLAuthorizationStatus status = self.manager.authorizationStatus;
    switch (status) {
        case kCLAuthorizationStatusNotDetermined:
            [self emit:@"permission|not_determined"];
            if (self.backgroundTracking) {
                [self emit:@"error|permission_denied|Grant location permission from the visible app before background tracking"];
                [self finish:@"permission_denied"];
                return;
            }
            if (![[NSBundle mainBundle] objectForInfoDictionaryKey:@"NSLocationWhenInUseUsageDescription"]) {
                [self emit:@"error|configuration|NSLocationWhenInUseUsageDescription is required"];
                [self finish:@"configuration"];
                return;
            }
            [self.manager requestWhenInUseAuthorization];
            return;
        case kCLAuthorizationStatusDenied:
            self.updating = NO;
            [self emit:@"permission|denied"];
            [self.manager stopUpdatingLocation];
            if (self.backgroundTracking) [self finish:@"permission_denied"];
            return;
        case kCLAuthorizationStatusRestricted:
            self.updating = NO;
            [self emit:@"permission|restricted"];
            [self.manager stopUpdatingLocation];
            if (self.backgroundTracking) [self finish:@"permission_restricted"];
            return;
        default:
            [self emit:self.manager.accuracyAuthorization == CLAccuracyAuthorizationReducedAccuracy
                ? @"permission|approximate" : @"permission|precise"];
            // Authorization is rechecked on resume; an existing background owner
            // needs neither another start nor another acquisition-ready event.
            if (self.backgroundTracking && self.updating && subscriptions[@(self.token)] == self) return;
            [self.manager startUpdatingLocation];
            self.updating = YES;
            [self emit:self.backgroundTracking ? @"ready|background|CoreLocation" : @"ready|foreground|CoreLocation"];
    }
}
- (void)locationManagerDidChangeAuthorization:(CLLocationManager *)manager { [self authorize]; }
- (void)background:(NSNotification *)notification {
    if (self.backgroundTracking) return;
    self.suspended = YES;
    self.updating = NO;
    [self.manager stopUpdatingLocation];
    [self emit:@"paused|"];
}
- (void)foreground:(NSNotification *)notification {
    self.suspended = NO;
    [self authorize];
}
- (void)locationManager:(CLLocationManager *)manager didUpdateLocations:(NSArray<CLLocation *> *)locations {
    for (CLLocation *location in locations) {
        if (!self.active || self.suspended || !validLocation(location)) continue;
        if (self.once && [location.timestamp compare:self.requestedAt] == NSOrderedAscending) continue;
        NSString *value = serialize(location);
        @synchronized(GpuiLocationSubscription.class) { lastPosition = value; lastPositionTime = location.timestamp; }
        [self emit:[@"position|" stringByAppendingString:value]];
        if (self.once) { [self finish:@"completed"]; break; }
    }
}
- (void)locationManager:(CLLocationManager *)manager didFailWithError:(NSError *)error {
    if (!self.active || self.suspended) return;
    BOOL coreLocation = [error.domain isEqualToString:kCLErrorDomain];
    if (coreLocation && error.code == kCLErrorLocationUnknown) return;
    BOOL denied = coreLocation && error.code == kCLErrorDenied;
    NSString *code = denied ? @"permission_denied" : @"native";
    [self emit:[NSString stringWithFormat:@"error|%@|%@", code, error.localizedDescription]];
    // A tracking subscription remains available for a later permission change.
    if (self.once || (self.backgroundTracking && denied)) [self finish:@"native_error"];
}
@end

static BOOL canStartBackground(UIApplicationState state, uint64_t previous) {
    if (state == UIApplicationStateActive) return YES;
    GpuiLocationSubscription *owner = subscriptions[@(previous)];
    return owner.active && owner.backgroundTracking && owner.updating;
}

void gpui_location_start(uint64_t token, double accuracy, double distance, bool once, uint64_t timeout_ms, bool background, uint64_t previous) {
    dispatch_async(dispatch_get_main_queue(), ^{
        if (!subscriptions) subscriptions = [NSMutableDictionary new];
        if (background && !canStartBackground(UIApplication.sharedApplication.applicationState, previous)) {
            gpui_location_event(token, "error|background_start|Start a new background watch from the active app");
            gpui_location_event(token, "end|background_start");
            return;
        }
        [subscriptions[@(token)] stop];
        if (!isfinite(distance) || distance < 0 || distance > FLT_MAX
            || (once && (timeout_ms < 1 || timeout_ms > 86400000))) {
            gpui_location_event(token, "error|configuration|Invalid distance filter or timeout");
            gpui_location_event(token, "end|configuration");
            return;
        }
        GpuiLocationSubscription *subscription = [GpuiLocationSubscription new];
        subscription.token = token;
        subscription.once = once;
        subscription.backgroundTracking = background;
        subscription.active = YES;
        subscription.suspended = !background && UIApplication.sharedApplication.applicationState == UIApplicationStateBackground;
        subscription.requestedAt = [NSDate date];
        subscription.manager = [CLLocationManager new];
        configureManager(subscription.manager, accuracy, distance);
        subscriptions[@(token)] = subscription;
        if (background) {
            NSArray *modes = [[NSBundle mainBundle] objectForInfoDictionaryKey:@"UIBackgroundModes"];
            if (![modes isKindOfClass:NSArray.class] || ![modes containsObject:@"location"]) {
                [subscription emit:@"error|configuration|UIBackgroundModes must include location"];
                [subscription finish:@"configuration"];
                return;
            }
            subscription.manager.allowsBackgroundLocationUpdates = YES;
            subscription.manager.showsBackgroundLocationIndicator = YES;
        }
        [[NSNotificationCenter defaultCenter] addObserver:subscription selector:@selector(background:) name:UIApplicationDidEnterBackgroundNotification object:nil];
        [[NSNotificationCenter defaultCenter] addObserver:subscription selector:@selector(foreground:) name:UIApplicationDidBecomeActiveNotification object:nil];
        subscription.manager.delegate = subscription;
        [subscription authorize];
        if (once) {
            __weak GpuiLocationSubscription *weakSubscription = subscription;
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)timeout_ms * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
                GpuiLocationSubscription *pending = weakSubscription;
                if (pending.active) {
                    [pending emit:@"error|timeout|No fresh location before deadline"];
                    [pending finish:@"timeout"];
                }
            });
        }
    });
}
void gpui_location_stop(uint64_t token) {
    dispatch_async(dispatch_get_main_queue(), ^{ [subscriptions[@(token)] stop]; });
}
bool gpui_location_enabled(void) { return [CLLocationManager locationServicesEnabled]; }
bool gpui_location_is_main_thread(void) { return [NSThread isMainThread]; }
void gpui_location_on_main(void *context, void (*callback)(void *)) {
    if ([NSThread isMainThread]) callback(context);
    else dispatch_async(dispatch_get_main_queue(), ^{ callback(context); });
}
// Developer probe queries after queued stop; never exposes the manager/pointer.
bool gpui_location_subscription_active(uint64_t token) {
    __block BOOL active = NO;
    void (^query)(void) = ^{ active = [subscriptions[@(token)] active]; };
    if ([NSThread isMainThread]) query();
    else dispatch_sync(dispatch_get_main_queue(), query);
    return active;
}
static void readNativeCache(CLLocationManager *manager) {
    CLLocation *location = manager.location;
    if (!validLocation(location)) return;
    @synchronized(GpuiLocationSubscription.class) {
        if (!lastPositionTime || [location.timestamp compare:lastPositionTime] == NSOrderedDescending) {
            lastPosition = serialize(location);
            lastPositionTime = location.timestamp;
        }
    }
}
bool gpui_location_last(char *buffer, size_t capacity) {
    void (^query)(void) = ^{
        CLLocationManager *manager = [CLLocationManager new];
        readNativeCache(manager); // No authorization request or update subscription.
        for (GpuiLocationSubscription *subscription in subscriptions.allValues) readNativeCache(subscription.manager);
    };
    if ([NSThread isMainThread]) query();
    else dispatch_sync(dispatch_get_main_queue(), query);
    @synchronized(GpuiLocationSubscription.class) {
        if (!lastPosition || strlen(lastPosition.UTF8String) + 1 > capacity) return false;
        strcpy(buffer, lastPosition.UTF8String);
        return true;
    }
}

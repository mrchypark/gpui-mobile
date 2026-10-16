// Synthetic delegate tests: no GPS/permission-prompt capability is claimed.
#import "../GpuiLocation.m"
#include <assert.h>
static NSMutableArray<NSString *> *events;
static void permissionDispatchCheck(void *context) {
    assert([NSThread isMainThread]);
    *(BOOL *)context = YES;
}
void gpui_location_event(uint64_t token, const char *event) { [events addObject:@(event)]; }

@interface TestManager : NSObject
@property(nonatomic) CLAuthorizationStatus authorizationStatus;
@property(nonatomic) CLAccuracyAuthorization accuracyAuthorization;
@property(nonatomic, weak) id delegate;
@property(nonatomic) int starts;
@property(nonatomic) int stops;
@property(nonatomic) double desiredAccuracy;
@property(nonatomic) double distanceFilter;
@property(nonatomic) BOOL pausesLocationUpdatesAutomatically;
@property(nonatomic) BOOL allowsBackgroundLocationUpdates;
@property(nonatomic) CLLocation *location;
- (void)startUpdatingLocation;
- (void)stopUpdatingLocation;
@end
@implementation TestManager
- (void)startUpdatingLocation { self.starts++; }
- (void)stopUpdatingLocation { self.stops++; }
@end
static GpuiLocationSubscription *make(uint64_t token, TestManager *manager) {
    GpuiLocationSubscription *subscription = [GpuiLocationSubscription new];
    subscription.active = YES;
    subscription.token = token;
    subscription.manager = (CLLocationManager *)manager;
    subscription.requestedAt = [NSDate date];
    subscriptions[@(token)] = subscription;
    return subscription;
}
static CLLocation *sample(NSDate *time) {
    return [[CLLocation alloc] initWithCoordinate:CLLocationCoordinate2DMake(37.5, 127)
        altitude:12 horizontalAccuracy:5 verticalAccuracy:3 timestamp:time];
}
int main(void) {
    @autoreleasepool {
        BOOL direct = NO;
        gpui_location_on_main(&direct, permissionDispatchCheck);
        assert(direct);
        __block BOOL handedOff = NO;
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{
            gpui_location_on_main(&handedOff, permissionDispatchCheck);
        });
        NSDate *dispatchDeadline = [NSDate dateWithTimeIntervalSinceNow:2];
        while (!handedOff && [dispatchDeadline timeIntervalSinceNow] > 0) {
            [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
        }
        assert(handedOff); // production permission handoff from a worker reaches UIKit main
        subscriptions = [NSMutableDictionary new];
        events = [NSMutableArray new];
        TestManager *manager = [TestManager new];
        manager.pausesLocationUpdatesAutomatically = YES;
        manager.allowsBackgroundLocationUpdates = YES;
        configureManager((CLLocationManager *)manager, kCLLocationAccuracyBest, 12);
        assert(!manager.pausesLocationUpdatesAutomatically && !manager.allowsBackgroundLocationUpdates);
        assert(manager.desiredAccuracy == kCLLocationAccuracyBest && manager.distanceFilter == 12);
        configureManager((CLLocationManager *)manager, kCLLocationAccuracyBest, 0);
        assert(manager.distanceFilter == kCLDistanceFilterNone);
        GpuiLocationSubscription *subscription = make(1, manager);
        assert(canStartBackground(UIApplicationStateActive, 0));
        assert(!canStartBackground(UIApplicationStateInactive, 0));
        assert(!canStartBackground(UIApplicationStateBackground, 0));
        subscription.backgroundTracking = YES;
        assert(!canStartBackground(UIApplicationStateBackground, 1)); // not started yet
        subscription.updating = YES;
        assert(canStartBackground(UIApplicationStateBackground, 1)); // exact live predecessor
        assert(!canStartBackground(UIApplicationStateBackground, 99)); // unrelated token
        subscription.active = NO;
        assert(!canStartBackground(UIApplicationStateBackground, 1));
        subscription.active = YES;
        subscription.backgroundTracking = NO;
        assert(!canStartBackground(UIApplicationStateBackground, 1)); // foreground owner is insufficient
        subscription.updating = NO;
        manager.authorizationStatus = kCLAuthorizationStatusDenied;
        [subscription authorize];
        assert([events.lastObject isEqual:@"permission|denied"] && manager.starts == 0);
        manager.authorizationStatus = kCLAuthorizationStatusAuthorizedWhenInUse;
        manager.accuracyAuthorization = CLAccuracyAuthorizationReducedAccuracy;
        [subscription authorize];
        assert([events[events.count - 2] isEqual:@"permission|approximate"] && manager.starts == 1);
        assert([events.lastObject isEqual:@"ready|foreground|CoreLocation"]);
        manager.accuracyAuthorization = CLAccuracyAuthorizationFullAccuracy;
        [subscription authorize];
        assert([events[events.count - 2] isEqual:@"permission|precise"]);
        [subscription background:nil];
        NSUInteger count = events.count;
        [subscription locationManager:(CLLocationManager *)manager didFailWithError:
            [NSError errorWithDomain:kCLErrorDomain code:kCLErrorDenied userInfo:nil]];
        assert(events.count == count && subscription.active);
        [subscription locationManager:(CLLocationManager *)manager didUpdateLocations:@[sample([NSDate date])]];
        assert(events.count == count);
        int foregroundStarts = manager.starts;
        [subscription foreground:nil];
        assert(manager.starts == foregroundStarts + 1 && subscriptions[@1] == subscription);
        [subscription locationManager:(CLLocationManager *)manager didUpdateLocations:@[sample([NSDate date])]];
        assert([events.lastObject hasPrefix:@"position|37.5|127|"]);
        [subscription stop];
        count = events.count;
        [subscription locationManager:(CLLocationManager *)manager didUpdateLocations:@[sample([NSDate date])]];
        assert(events.count == count && subscriptions[@1] == nil);
        [subscription locationManager:(CLLocationManager *)manager didFailWithError:
            [NSError errorWithDomain:kCLErrorDomain code:kCLErrorDenied userInfo:nil]];
        assert(events.count == count);

        GpuiLocationSubscription *replacement = make(1, manager);
        [subscription stop]; // A late callback from the old request cannot remove the replacement.
        assert(subscriptions[@1] == replacement);
        [replacement stop];

        GpuiLocationSubscription *backgroundWatch = make(4, manager);
        backgroundWatch.backgroundTracking = YES;
        manager.authorizationStatus = kCLAuthorizationStatusAuthorizedWhenInUse;
        [backgroundWatch authorize];
        assert([events.lastObject isEqual:@"ready|background|CoreLocation"]);
        assert(backgroundWatch.updating);
        int startsBeforeBackground = manager.starts;
        [backgroundWatch background:nil];
        assert(!backgroundWatch.suspended && backgroundWatch.updating);
        [backgroundWatch locationManager:(CLLocationManager *)manager didUpdateLocations:@[sample([NSDate date])]];
        assert([events.lastObject hasPrefix:@"position|"]);
        [backgroundWatch foreground:nil];
        assert(manager.starts == startsBeforeBackground);
        count = events.count;
        [backgroundWatch locationManager:(CLLocationManager *)manager didFailWithError:
            [NSError errorWithDomain:kCLErrorDomain code:kCLErrorLocationUnknown userInfo:nil]];
        assert(events.count == count && backgroundWatch.active && backgroundWatch.updating);
        [backgroundWatch locationManager:(CLLocationManager *)manager didFailWithError:
            [NSError errorWithDomain:@"foreign" code:kCLErrorDenied userInfo:nil]];
        assert([events.lastObject hasPrefix:@"error|native|"] && backgroundWatch.active);
        manager.authorizationStatus = kCLAuthorizationStatusDenied;
        [backgroundWatch authorize];
        assert(!backgroundWatch.active && !backgroundWatch.updating && subscriptions[@4] == nil);
        assert([events.lastObject isEqual:@"end|permission_denied"]);
        assert(!canStartBackground(UIApplicationStateBackground, 4));

        GpuiLocationSubscription *once = make(2, manager);
        once.once = YES;
        lastPosition = nil;
        lastPositionTime = nil;
        manager.location = sample([NSDate dateWithTimeIntervalSinceNow:-60]);
        count = events.count;
        int cacheStarts = manager.starts;
        readNativeCache((CLLocationManager *)manager);
        assert(lastPosition != nil && events.count == count && manager.starts == cacheStarts);
        [once locationManager:(CLLocationManager *)manager didUpdateLocations:@[manager.location]];
        assert(once.active && events.count == count); // cache never satisfies fresh one-shot
        NSString *savedCache = lastPosition;
        manager.location = nil;
        readNativeCache((CLLocationManager *)manager);
        assert([lastPosition isEqual:savedCache]);
        [once locationManager:(CLLocationManager *)manager didFailWithError:
            [NSError errorWithDomain:kCLErrorDomain code:kCLErrorLocationUnknown userInfo:nil]];
        assert(once.active && events.count == count);
        count = events.count;
        CLLocation *invalidFix = [[CLLocation alloc] initWithCoordinate:CLLocationCoordinate2DMake(91, 127)
            altitude:0 horizontalAccuracy:5 verticalAccuracy:3 timestamp:[NSDate dateWithTimeIntervalSinceNow:1]];
        manager.location = invalidFix;
        readNativeCache((CLLocationManager *)manager);
        assert([lastPosition isEqual:savedCache]);
        [once locationManager:(CLLocationManager *)manager didUpdateLocations:@[invalidFix]];
        assert(events.count == count && once.active);
        [once locationManager:(CLLocationManager *)manager didUpdateLocations:@[sample([NSDate dateWithTimeIntervalSinceNow:-60])]];
        assert(events.count == count && once.active);
        [once locationManager:(CLLocationManager *)manager didUpdateLocations:@[sample([NSDate dateWithTimeIntervalSinceNow:1])]];
        assert(events.count == count + 2 && !once.active && subscriptions[@2] == nil);
        assert([events.lastObject isEqual:@"end|completed"]);

        GpuiLocationSubscription *foreign = make(5, manager);
        foreign.once = YES;
        [foreign locationManager:(CLLocationManager *)manager didFailWithError:
            [NSError errorWithDomain:@"foreign" code:kCLErrorLocationUnknown userInfo:nil]];
        assert(!foreign.active && [events[events.count - 2] hasPrefix:@"error|native|"]);

        GpuiLocationSubscription *invalid = make(3, manager);
        manager.authorizationStatus = kCLAuthorizationStatusNotDetermined;
        [invalid authorize]; // This test executable has no location usage description.
        assert([events[events.count - 2] hasPrefix:@"error|configuration|"]);
        assert([events.lastObject isEqual:@"end|configuration"] && !invalid.active);
        puts("PASS: permission recovery, pause/resume, cancellation, stale callback isolation, fresh one-shot, terminal configuration error");
    }
    return 0;
}

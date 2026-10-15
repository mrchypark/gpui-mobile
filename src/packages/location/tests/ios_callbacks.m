// Synthetic delegate tests: no GPS/permission-prompt capability is claimed.
#import "../GpuiLocation.m"
#include <assert.h>
static NSMutableArray<NSString *> *events;
void gpui_location_event(uint64_t token, const char *event) { [events addObject:@(event)]; }

@interface TestManager : NSObject
@property(nonatomic) CLAuthorizationStatus authorizationStatus;
@property(nonatomic) CLAccuracyAuthorization accuracyAuthorization;
@property(nonatomic, weak) id delegate;
@property(nonatomic) int starts;
@property(nonatomic) int stops;
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
        subscriptions = [NSMutableDictionary new];
        events = [NSMutableArray new];
        TestManager *manager = [TestManager new];
        GpuiLocationSubscription *subscription = make(1, manager);
        manager.authorizationStatus = kCLAuthorizationStatusDenied;
        [subscription authorize];
        assert([events.lastObject isEqual:@"permission|denied"] && manager.starts == 0);
        manager.authorizationStatus = kCLAuthorizationStatusAuthorizedWhenInUse;
        manager.accuracyAuthorization = CLAccuracyAuthorizationReducedAccuracy;
        [subscription authorize];
        assert([events.lastObject isEqual:@"permission|approximate"] && manager.starts == 1);
        manager.accuracyAuthorization = CLAccuracyAuthorizationFullAccuracy;
        [subscription authorize];
        assert([events.lastObject isEqual:@"permission|precise"]);
        [subscription background:nil];
        NSUInteger count = events.count;
        [subscription locationManager:(CLLocationManager *)manager didUpdateLocations:@[sample([NSDate date])]];
        assert(events.count == count);
        [subscription foreground:nil];
        [subscription locationManager:(CLLocationManager *)manager didUpdateLocations:@[sample([NSDate date])]];
        assert([events.lastObject hasPrefix:@"position|37.5|127|"]);
        [subscription stop];
        count = events.count;
        [subscription locationManager:(CLLocationManager *)manager didUpdateLocations:@[sample([NSDate date])]];
        assert(events.count == count && subscriptions[@1] == nil);

        GpuiLocationSubscription *replacement = make(1, manager);
        [subscription stop]; // A late callback from the old request cannot remove the replacement.
        assert(subscriptions[@1] == replacement);
        [replacement stop];

        GpuiLocationSubscription *once = make(2, manager);
        once.once = YES;
        count = events.count;
        [once locationManager:(CLLocationManager *)manager didUpdateLocations:@[sample([NSDate dateWithTimeIntervalSinceNow:-60])]];
        assert(events.count == count && once.active);
        [once locationManager:(CLLocationManager *)manager didUpdateLocations:@[sample([NSDate dateWithTimeIntervalSinceNow:1])]];
        assert(events.count == count + 2 && !once.active && subscriptions[@2] == nil);
        assert([events.lastObject isEqual:@"end|completed"]);

        GpuiLocationSubscription *invalid = make(3, manager);
        manager.authorizationStatus = kCLAuthorizationStatusNotDetermined;
        [invalid authorize]; // This test executable has no location usage description.
        assert([events[events.count - 2] hasPrefix:@"error|configuration|"]);
        assert([events.lastObject isEqual:@"end|configuration"] && !invalid.active);
        puts("PASS: permission recovery, pause/resume, cancellation, stale callback isolation, fresh one-shot, terminal configuration error");
    }
    return 0;
}

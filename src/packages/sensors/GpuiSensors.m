#import <CoreMotion/CoreMotion.h>
#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#include <math.h>

@interface GpuiSensorState : NSObject
@property(nonatomic, strong) CMMotionManager *motion;
@property(nonatomic, strong) CMAltimeter *altimeter;
@property(nonatomic) BOOL pressureActive;
@property(nonatomic) BOOL hasPressure;
@property(nonatomic) double pressure;
@property(nonatomic) NSUInteger generation;
- (void)stop;
- (void)applicationStopped:(NSNotification *)notification;
@end
@implementation GpuiSensorState
- (instancetype)init {
    if ((self = [super init])) {
        _motion = [CMMotionManager new];
        _motion.accelerometerUpdateInterval = 0.1;
        _motion.gyroUpdateInterval = 0.1;
        _motion.magnetometerUpdateInterval = 0.1;
        _altimeter = [CMAltimeter new];
        [[NSNotificationCenter defaultCenter] addObserver:self selector:@selector(applicationStopped:)
            name:UIApplicationDidEnterBackgroundNotification object:nil];
        [[NSNotificationCenter defaultCenter] addObserver:self selector:@selector(applicationStopped:)
            name:UIApplicationWillTerminateNotification object:nil];
    }
    return self;
}
- (void)applicationStopped:(NSNotification *)notification {
    (void)notification;
    [self stop];
}
- (void)stop {
    self.generation++;
    [self.motion stopAccelerometerUpdates];
    [self.motion stopGyroUpdates];
    [self.motion stopMagnetometerUpdates];
    [self.altimeter stopRelativeAltitudeUpdates];
    self.pressureActive = NO;
    self.hasPressure = NO;
}
- (void)dealloc {
    [[NSNotificationCenter defaultCenter] removeObserver:self];
    [_motion stopAccelerometerUpdates];
    [_motion stopGyroUpdates];
    [_motion stopMagnetometerUpdates];
    [_altimeter stopRelativeAltitudeUpdates];
}
@end

static void on_main(void (^work)(void)) {
    if ([NSThread isMainThread]) work();
    else dispatch_sync(dispatch_get_main_queue(), work);
}
static GpuiSensorState *state(void) {
    static GpuiSensorState *value;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ value = [GpuiSensorState new]; });
    return value;
}
unsigned int gpui_sensors_available(void) {
    __block unsigned int bits = 0;
    on_main(^{
        CMMotionManager *motion = state().motion;
        bits = (motion.accelerometerAvailable ? 1 : 0) |
               (motion.gyroAvailable ? 2 : 0) | (motion.magnetometerAvailable ? 4 : 0) |
               ([CMAltimeter isRelativeAltitudeAvailable] ? 8 : 0);
    });
    return bits;
}
// type: accelerometer=0, gyroscope=1, magnetometer=2, pressure=3.
bool gpui_sensors_read(unsigned int type, double *xyz) {
    if (!xyz || type > 3) return false;
    __block BOOL valid = NO;
    on_main(^{
        GpuiSensorState *s = state();
        if ([UIApplication sharedApplication].applicationState == UIApplicationStateBackground) {
            [s stop];
            return;
        }
        CMMotionManager *m = s.motion;
        if (type == 0 && m.accelerometerAvailable) {
            if (!m.accelerometerActive) { [m startAccelerometerUpdates]; return; }
            CMAccelerometerData *data = m.accelerometerData;
            if (!data) return;
            CMAcceleration v = data.acceleration;
            xyz[0] = v.x * 9.80665; xyz[1] = v.y * 9.80665; xyz[2] = v.z * 9.80665;
            valid = YES;
        } else if (type == 1 && m.gyroAvailable) {
            if (!m.gyroActive) { [m startGyroUpdates]; return; }
            CMGyroData *data = m.gyroData;
            if (!data) return;
            CMRotationRate v = data.rotationRate;
            xyz[0] = v.x; xyz[1] = v.y; xyz[2] = v.z;
            valid = YES;
        } else if (type == 2 && m.magnetometerAvailable) {
            if (!m.magnetometerActive) { [m startMagnetometerUpdates]; return; }
            CMMagnetometerData *data = m.magnetometerData;
            if (!data) return;
            CMMagneticField v = data.magneticField;
            xyz[0] = v.x; xyz[1] = v.y; xyz[2] = v.z;
            valid = YES;
        } else if (type == 3 && [CMAltimeter isRelativeAltitudeAvailable]) {
            if (@available(iOS 11.0, *)) {
                CMAuthorizationStatus authorization = [CMAltimeter authorizationStatus];
                if (authorization == CMAuthorizationStatusDenied || authorization == CMAuthorizationStatusRestricted) {
                    [s stop];
                    return;
                }
            }
            // CoreMotion requires a usage description; avoid the OS terminating a misconfigured host.
            NSString *usage = [[NSBundle mainBundle] objectForInfoDictionaryKey:@"NSMotionUsageDescription"];
            if (usage.length == 0) return;
            if (!s.pressureActive) {
                s.pressureActive = YES;
                NSUInteger generation = ++s.generation;
                __weak GpuiSensorState *weakState = s;
                [s.altimeter startRelativeAltitudeUpdatesToQueue:[NSOperationQueue mainQueue]
                    withHandler:^(CMAltitudeData *data, NSError *error) {
                        GpuiSensorState *current = weakState;
                        if (!current || current.generation != generation || !current.pressureActive) return;
                        if (error || !data) {
                            NSLog(@"GpuiSensors pressure unavailable: %@", error);
                            [current stop];
                            return;
                        }
                        // Preserve native kPa; Rust converts to the public hPa contract.
                        double pressure = data.pressure.doubleValue;
                        current.hasPressure = isfinite(pressure) && pressure > 0;
                        current.pressure = pressure;
                    }];
            }
            if (!s.hasPressure) return;
            xyz[0] = s.pressure; xyz[1] = 0; xyz[2] = 0;
            valid = YES;
        }
    });
    return valid && isfinite(xyz[0]) && isfinite(xyz[1]) && isfinite(xyz[2]);
}

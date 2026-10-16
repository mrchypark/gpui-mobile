// Run on the main thread in an isolated iOS host linked to the real native bridge.
// Uses real CoreMotion plus the same lifecycle notification observed in production.
#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#include <assert.h>
#include <stdbool.h>
extern unsigned int gpui_sensors_available(void);
extern bool gpui_sensors_read(unsigned int, double *);
void gpui_test_sensors_native_lifecycle(void) {
    assert([NSThread isMainThread]);
    double values[3];
    assert(!gpui_sensors_read(4, values));
    assert(!gpui_sensors_read(0, NULL));
    unsigned int available = gpui_sensors_available();
    for (unsigned int sensor = 0; sensor < 4; sensor++) {
        if (!(available & (1u << sensor))) assert(!gpui_sensors_read(sensor, values));
        else (void)gpui_sensors_read(sensor, values);
    }
    [[NSNotificationCenter defaultCenter] postNotificationName:UIApplicationDidEnterBackgroundNotification object:nil];
    // Every stream has stopped and cleared. A restart must not expose the previous sample.
    for (unsigned int sensor = 0; sensor < 4; sensor++) assert(!gpui_sensors_read(sensor, values));
    [[NSNotificationCenter defaultCenter] postNotificationName:UIApplicationWillTerminateNotification object:nil];
}

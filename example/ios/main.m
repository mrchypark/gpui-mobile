// GPUI iOS Example - Main Entry Point
//
// This is a minimal iOS app that demonstrates GPUI running on iOS.
// When USE_GPUI_RUST is defined, it links against the GPUI Rust static library
// and uses GPUI for all rendering.

#import <UIKit/UIKit.h>
#import <Metal/Metal.h>
#import <QuartzCore/QuartzCore.h>
#import <TargetConditionals.h>
#if TARGET_OS_SIMULATOR
#include <dlfcn.h>
#endif

// Define USE_GPUI_RUST to enable Rust GPUI integration
// This requires linking against libgpui.a
#ifdef USE_GPUI_RUST
#import "gpui_ios.h"
#endif

#ifndef USE_GPUI_RUST
// Fallback Metal view for when GPUI is not linked
@interface GPUIMetalView : UIView
@property (nonatomic, strong) id<MTLDevice> device;
@property (nonatomic, strong) id<MTLCommandQueue> commandQueue;
@end

@implementation GPUIMetalView

+ (Class)layerClass {
    return [CAMetalLayer class];
}

- (instancetype)initWithFrame:(CGRect)frame {
    self = [super initWithFrame:frame];
    if (self) {
        [self setupMetal];
    }
    return self;
}

- (void)setupMetal {
    self.device = MTLCreateSystemDefaultDevice();
    if (!self.device) {
        NSLog(@"Metal is not supported on this device");
        return;
    }

    CAMetalLayer *metalLayer = (CAMetalLayer *)self.layer;
    metalLayer.device = self.device;
    metalLayer.pixelFormat = MTLPixelFormatBGRA8Unorm;
    metalLayer.framebufferOnly = YES;
    metalLayer.contentsScale = [UIScreen mainScreen].scale;

    self.commandQueue = [self.device newCommandQueue];

    NSLog(@"Metal initialized successfully with device: %@", self.device.name);
}

- (void)drawRect:(CGRect)rect {
    CAMetalLayer *metalLayer = (CAMetalLayer *)self.layer;
    id<CAMetalDrawable> drawable = [metalLayer nextDrawable];
    if (!drawable) return;

    MTLRenderPassDescriptor *passDescriptor = [MTLRenderPassDescriptor renderPassDescriptor];
    passDescriptor.colorAttachments[0].texture = drawable.texture;
    passDescriptor.colorAttachments[0].loadAction = MTLLoadActionClear;
    passDescriptor.colorAttachments[0].storeAction = MTLStoreActionStore;
    // Catppuccin Mocha base color
    passDescriptor.colorAttachments[0].clearColor = MTLClearColorMake(0.118, 0.118, 0.180, 1.0);

    id<MTLCommandBuffer> commandBuffer = [self.commandQueue commandBuffer];
    id<MTLRenderCommandEncoder> encoder = [commandBuffer renderCommandEncoderWithDescriptor:passDescriptor];
    [encoder endEncoding];

    [commandBuffer presentDrawable:drawable];
    [commandBuffer commit];
}

@end

// Fallback View Controller for non-GPUI mode
@interface GPUIFallbackViewController : UIViewController
@property (nonatomic, strong) GPUIMetalView *metalView;
@property (nonatomic, strong) UILabel *statusLabel;
@property (nonatomic, strong) CADisplayLink *displayLink;
@end

@implementation GPUIFallbackViewController

- (void)viewDidLoad {
    [super viewDidLoad];

    self.metalView = [[GPUIMetalView alloc] initWithFrame:self.view.bounds];
    self.metalView.autoresizingMask = UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight;
    [self.view addSubview:self.metalView];

    self.statusLabel = [[UILabel alloc] init];
    self.statusLabel.text = @"GPUI iOS\nMetal Fallback Mode";
    self.statusLabel.numberOfLines = 0;
    self.statusLabel.textAlignment = NSTextAlignmentCenter;
    self.statusLabel.textColor = [UIColor colorWithRed:0.804 green:0.839 blue:0.957 alpha:1.0];
    self.statusLabel.font = [UIFont systemFontOfSize:24 weight:UIFontWeightBold];
    self.statusLabel.translatesAutoresizingMaskIntoConstraints = NO;
    [self.view addSubview:self.statusLabel];

    [NSLayoutConstraint activateConstraints:@[
        [self.statusLabel.centerXAnchor constraintEqualToAnchor:self.view.centerXAnchor],
        [self.statusLabel.centerYAnchor constraintEqualToAnchor:self.view.centerYAnchor],
    ]];

    self.displayLink = [CADisplayLink displayLinkWithTarget:self selector:@selector(render)];
    [self.displayLink addToRunLoop:[NSRunLoop mainRunLoop] forMode:NSRunLoopCommonModes];

    NSLog(@"GPUI iOS Fallback Mode Started");
}

- (void)render {
    [self.metalView setNeedsDisplay];
}

- (void)viewDidLayoutSubviews {
    [super viewDidLayoutSubviews];
    CAMetalLayer *metalLayer = (CAMetalLayer *)self.metalView.layer;
    CGFloat scale = [UIScreen mainScreen].scale;
    metalLayer.drawableSize = CGSizeMake(self.metalView.bounds.size.width * scale,
                                          self.metalView.bounds.size.height * scale);
}

- (UIStatusBarStyle)preferredStatusBarStyle {
    return UIStatusBarStyleLightContent;
}

- (void)dealloc {
    [self.displayLink invalidate];
}

@end
#endif // !USE_GPUI_RUST

// App Delegate
@interface GPUIAppDelegate : UIResponder <UIApplicationDelegate>
@property (nonatomic, strong) UIWindow *window;
#ifdef USE_GPUI_RUST
@property (nonatomic, assign) void *gpuiApp;
@property (nonatomic, assign) void *gpuiWindow;
@property (nonatomic) BOOL awaitingInitialUserActivity;
@property (nonatomic, strong) CADisplayLink *displayLink;
#endif
@end

@implementation GPUIAppDelegate

- (BOOL)application:(UIApplication *)application didFinishLaunchingWithOptions:(NSDictionary *)launchOptions {
    NSLog(@"GPUI iOS Application Launching...");

#ifdef USE_GPUI_RUST
    NSURL *initialURL = launchOptions[UIApplicationLaunchOptionsURLKey];
    NSDictionary *activities = launchOptions[UIApplicationLaunchOptionsUserActivityDictionaryKey];
    for (id value in activities.allValues) {
        if ([value isKindOfClass:NSUserActivity.class] &&
            [((NSUserActivity *)value).activityType isEqualToString:NSUserActivityTypeBrowsingWeb]) {
            initialURL = ((NSUserActivity *)value).webpageURL;
            break;
        }
    }
    if (initialURL == nil) {
        NSArray<NSString *> *arguments = NSProcessInfo.processInfo.arguments;
        if (arguments.count > 1) {
            NSURL *argumentURL = [NSURL URLWithString:arguments[1]];
            if ([argumentURL.scheme.lowercaseString isEqualToString:@"gpui"]) {
                initialURL = argumentURL;
            }
        }
    }
    self.awaitingInitialUserActivity = initialURL == nil &&
        [activities[UIApplicationLaunchOptionsUserActivityTypeKey] isEqual:NSUserActivityTypeBrowsingWeb];
    if (!self.awaitingInitialUserActivity)
        gpui_ios_record_initial_url((__bridge void *)initialURL.absoluteString);
    // Register the example app's root view (Router), then start GPUI.
    // gpui_ios_register_app() is defined in the example crate and sets up
    // the callback that creates the Router with all screens + demos.
    // gpui_ios_run_demo() is defined in gpui-mobile and starts the run loop.
    NSLog(@"Starting GPUI app...");
    gpui_ios_register_app();
    gpui_ios_run_demo();
    NSLog(@"GPUI app initialized");

    // Get the GPUI window pointer that was created
    self.gpuiWindow = gpui_ios_get_window();
    if (self.gpuiWindow) {
        NSLog(@"Got GPUI window pointer: %p", self.gpuiWindow);

        // Setup CADisplayLink to drive rendering
        self.displayLink = [CADisplayLink displayLinkWithTarget:self selector:@selector(renderFrame)];
        [self.displayLink addToRunLoop:[NSRunLoop mainRunLoop] forMode:NSRunLoopCommonModes];
        NSLog(@"CADisplayLink started for GPUI rendering");
    } else {
        NSLog(@"Warning: No GPUI window was created");
    }

    NSLog(@"GPUI iOS Application Launched with Rust integration");
#else
    // Fallback mode: create our own window with demo UI
    self.window = [[UIWindow alloc] initWithFrame:[UIScreen mainScreen].bounds];
    self.window.rootViewController = [[GPUIFallbackViewController alloc] init];
    [self.window makeKeyAndVisible];
    NSLog(@"GPUI iOS Application Launched in fallback mode");
#endif

    return YES;
}

#ifdef USE_GPUI_RUST
- (void)renderFrame {
    if (self.gpuiWindow) {
        gpui_ios_request_frame(self.gpuiWindow);
    }
}
#endif

- (void)applicationWillEnterForeground:(UIApplication *)application {
    NSLog(@"GPUI iOS: Will enter foreground");
#ifdef USE_GPUI_RUST
    gpui_ios_will_enter_foreground(self.gpuiApp);

    // Resume display link when coming to foreground
    if (!self.displayLink && self.gpuiWindow) {
        self.displayLink = [CADisplayLink displayLinkWithTarget:self selector:@selector(renderFrame)];
        [self.displayLink addToRunLoop:[NSRunLoop mainRunLoop] forMode:NSRunLoopCommonModes];
    }
#endif
}

- (void)applicationDidBecomeActive:(UIApplication *)application {
    NSLog(@"GPUI iOS: Did become active");
#ifdef USE_GPUI_RUST
    gpui_ios_did_become_active(self.gpuiApp);
#if TARGET_OS_SIMULATOR
    // Opt-in debug probe; release archives do not export this symbol.
    if ([[[NSProcessInfo processInfo] environment][@"GPUI_LOCATION_PROBE"] isEqualToString:@"1"]) {
        void (*probe)(void) = (void (*)(void))dlsym(RTLD_DEFAULT, "gpui_location_start_simulator_probe");
        if (probe) {
            probe();
        } else {
            NSLog(@"GPS_SIM_PROBE unavailable in this build");
        }
    }
#endif
#endif
}

- (void)applicationWillResignActive:(UIApplication *)application {
    NSLog(@"GPUI iOS: Will resign active");
#ifdef USE_GPUI_RUST
    gpui_ios_will_resign_active(self.gpuiApp);
#endif
}

- (void)applicationDidEnterBackground:(UIApplication *)application {
    NSLog(@"GPUI iOS: Did enter background");
#ifdef USE_GPUI_RUST
    gpui_ios_did_enter_background(self.gpuiApp);

    // Pause display link when going to background to save power
    if (self.displayLink) {
        [self.displayLink invalidate];
        self.displayLink = nil;
    }
#endif
}

- (BOOL)application:(UIApplication *)application openURL:(NSURL *)url options:(NSDictionary<UIApplicationOpenURLOptionsKey, id> *)options {
    NSLog(@"GPUI iOS: Open URL: %@", url);
#ifdef USE_GPUI_RUST
    NSString *urlString = [url absoluteString];
    gpui_ios_handle_open_url((__bridge void *)urlString);
#endif
    return YES;
}

- (BOOL)application:(UIApplication *)application continueUserActivity:(NSUserActivity *)activity
        restorationHandler:(void (^)(NSArray<id<UIUserActivityRestoring>> *))restorationHandler {
    if (![activity.activityType isEqualToString:NSUserActivityTypeBrowsingWeb] || !activity.webpageURL)
        return NO;
#ifdef USE_GPUI_RUST
    NSString *url = activity.webpageURL.absoluteString;
    if (self.awaitingInitialUserActivity) {
        self.awaitingInitialUserActivity = NO;
        gpui_ios_record_initial_url((__bridge void *)url);
    }
    gpui_ios_handle_open_url((__bridge void *)url);
#endif
    return YES;
}

- (void)application:(UIApplication *)application didFailToContinueUserActivityWithType:(NSString *)type
        error:(NSError *)error {
#ifdef USE_GPUI_RUST
    if (self.awaitingInitialUserActivity && [type isEqualToString:NSUserActivityTypeBrowsingWeb]) {
        self.awaitingInitialUserActivity = NO;
        gpui_ios_record_initial_url(NULL);
    }
#endif
}

- (void)applicationWillTerminate:(UIApplication *)application {
    NSLog(@"GPUI iOS: Will terminate");
#ifdef USE_GPUI_RUST
    if (self.displayLink) {
        [self.displayLink invalidate];
        self.displayLink = nil;
    }
    gpui_ios_will_terminate(self.gpuiApp);
#endif
}

@end

// Main entry point
int main(int argc, char * argv[]) {
    @autoreleasepool {
        return UIApplicationMain(argc, argv, nil, NSStringFromClass([GPUIAppDelegate class]));
    }
}

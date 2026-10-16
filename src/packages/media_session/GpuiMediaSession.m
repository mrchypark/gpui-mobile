#import <AVFoundation/AVFoundation.h>
#import <MediaPlayer/MediaPlayer.h>
#import <Foundation/Foundation.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <math.h>

extern bool gpui_media_action(uint32_t action);
extern bool gpui_media_seek(double seconds);

static BOOL initialized;
static NSMutableDictionary *metadata;
static NSArray<MPRemoteCommand *> *commands;
static NSMutableArray *tokens;
static NSMutableArray<NSNumber *> *enabledBefore;
static NSString *previousCategory;
static NSString *previousMode;
static AVAudioSessionCategoryOptions previousOptions;
static NSUInteger generation;

static void on_main(void (^work)(void)) {
    if ([NSThread isMainThread]) work();
    else dispatch_sync(dispatch_get_main_queue(), work);
}
static bool fail(char *error, size_t capacity, NSString *message) {
    if (error && capacity) snprintf(error, capacity, "%s", (message ?: @"Media session failed").UTF8String);
    return false;
}
static void remove_commands(void) {
    for (NSUInteger i = 0; i < tokens.count; i++) {
        [commands[i] removeTarget:tokens[i]];
        commands[i].enabled = enabledBefore[i].boolValue;
    }
    tokens = nil;
    enabledBefore = nil;
    commands = nil;
}

bool gpui_media_init(char *error, size_t capacity) {
    __block bool success = true;
    on_main(^{
        if (initialized) return;
        AVAudioSession *audio = [AVAudioSession sharedInstance];
        previousCategory = audio.category;
        previousMode = audio.mode;
        previousOptions = audio.categoryOptions;
        NSError *failure = nil;
        if (![audio setCategory:AVAudioSessionCategoryPlayback mode:AVAudioSessionModeDefault
            options:0 error:&failure] || ![audio setActive:YES error:&failure]) {
            [audio setCategory:previousCategory mode:previousMode options:previousOptions error:nil];
            success = fail(error, capacity, failure.localizedDescription);
            return;
        }
        MPRemoteCommandCenter *center = [MPRemoteCommandCenter sharedCommandCenter];
        commands = @[center.playCommand, center.pauseCommand, center.stopCommand,
            center.nextTrackCommand, center.previousTrackCommand, center.changePlaybackPositionCommand];
        tokens = [NSMutableArray new];
        enabledBefore = [NSMutableArray new];
        NSUInteger currentGeneration = ++generation;
        for (NSUInteger i = 0; i < commands.count; i++) {
            MPRemoteCommand *command = commands[i];
            [enabledBefore addObject:@(command.enabled)];
            id token = [command addTargetWithHandler:^MPRemoteCommandHandlerStatus(MPRemoteCommandEvent *event) {
                __block BOOL handled = NO;
                on_main(^{
                    if (!initialized || currentGeneration != generation) return;
                    if (i == 5) {
                        if ([event isKindOfClass:[MPChangePlaybackPositionCommandEvent class]]) {
                            handled = gpui_media_seek(((MPChangePlaybackPositionCommandEvent *)event).positionTime);
                        }
                    } else {
                        handled = gpui_media_action((uint32_t)i);
                    }
                });
                return handled ? MPRemoteCommandHandlerStatusSuccess : MPRemoteCommandHandlerStatusCommandFailed;
            }];
            if (!token) {
                remove_commands();
                [audio setActive:NO withOptions:AVAudioSessionSetActiveOptionNotifyOthersOnDeactivation error:nil];
                [audio setCategory:previousCategory mode:previousMode options:previousOptions error:nil];
                success = fail(error, capacity, @"Remote command registration failed");
                return;
            }
            [tokens addObject:token];
            command.enabled = YES;
        }
        metadata = [NSMutableDictionary new];
        initialized = YES;
    });
    return success;
}

bool gpui_media_metadata(const char *title, const char *artist, uint64_t duration_ms,
                         char *error, size_t capacity) {
    if (!title || !artist) return fail(error, capacity, @"Missing title or artist");
    NSString *titleString = [NSString stringWithUTF8String:title];
    NSString *artistString = [NSString stringWithUTF8String:artist];
    if (!titleString || !artistString) return fail(error, capacity, @"Invalid UTF-8 metadata");
    __block bool success = true;
    on_main(^{
        if (!initialized) { success = fail(error, capacity, @"Media session is not initialized"); return; }
        metadata[MPMediaItemPropertyTitle] = titleString;
        metadata[MPMediaItemPropertyArtist] = artistString;
        metadata[MPMediaItemPropertyPlaybackDuration] = @((double)duration_ms / 1000.0);
        [MPNowPlayingInfoCenter defaultCenter].nowPlayingInfo = metadata;
    });
    return success;
}

bool gpui_media_playback(bool playing, uint64_t position_ms, float speed,
                         char *error, size_t capacity) {
    if (!isfinite(speed) || speed < 0) return fail(error, capacity, @"Invalid playback speed");
    __block bool success = true;
    on_main(^{
        if (!initialized) { success = fail(error, capacity, @"Media session is not initialized"); return; }
        metadata[MPNowPlayingInfoPropertyElapsedPlaybackTime] = @((double)position_ms / 1000.0);
        metadata[MPNowPlayingInfoPropertyPlaybackRate] = @(playing ? speed : 0.0f);
        metadata[MPNowPlayingInfoPropertyDefaultPlaybackRate] = @(speed);
        [MPNowPlayingInfoCenter defaultCenter].nowPlayingInfo = metadata;
    });
    return success;
}

bool gpui_media_release(char *error, size_t capacity) {
    __block bool success = true;
    on_main(^{
        if (!initialized) return;
        initialized = NO;
        generation++;
        remove_commands();
        [MPNowPlayingInfoCenter defaultCenter].nowPlayingInfo = nil;
        metadata = nil;
        AVAudioSession *audio = [AVAudioSession sharedInstance];
        NSError *deactivateError = nil;
        NSError *restoreError = nil;
        BOOL deactivated = [audio setActive:NO withOptions:AVAudioSessionSetActiveOptionNotifyOthersOnDeactivation
            error:&deactivateError];
        BOOL restored = [audio setCategory:previousCategory mode:previousMode options:previousOptions error:&restoreError];
        previousCategory = nil;
        previousMode = nil;
        if (!deactivated || !restored) success = fail(error, capacity,
            (deactivateError ?: restoreError).localizedDescription);
    });
    return success;
}

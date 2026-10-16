// Run in an iOS test host linked with the real gpui-mobile static library.
// This exercises MPNowPlayingInfoCenter; it does not simulate remote commands.
#import <Foundation/Foundation.h>
#import <MediaPlayer/MediaPlayer.h>
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <math.h>
extern bool gpui_media_init(char *, size_t);
extern bool gpui_media_metadata(const char *, const char *, uint64_t, char *, size_t);
extern bool gpui_media_playback(bool, uint64_t, float, char *, size_t);
extern bool gpui_media_release(char *, size_t);
void gpui_test_media_native_state(void) {
    assert([NSThread isMainThread]);
    char error[512] = {0};
    assert(gpui_media_release(error, sizeof error));
    assert(!gpui_media_metadata("before init", "", 0, error, sizeof error));
    assert(gpui_media_init(error, sizeof error));
    assert(gpui_media_init(error, sizeof error));
    assert(gpui_media_metadata("Title", "Artist", 90000, error, sizeof error));
    assert(gpui_media_playback(true, 1250, 1.5f, error, sizeof error));
    NSDictionary *info = [MPNowPlayingInfoCenter defaultCenter].nowPlayingInfo;
    assert([info[MPMediaItemPropertyTitle] isEqual:@"Title"]);
    assert([info[MPMediaItemPropertyArtist] isEqual:@"Artist"]);
    assert([info[MPMediaItemPropertyPlaybackDuration] doubleValue] == 90.0);
    assert([info[MPNowPlayingInfoPropertyElapsedPlaybackTime] doubleValue] == 1.25);
    assert([info[MPNowPlayingInfoPropertyPlaybackRate] doubleValue] == 1.5);
    assert(gpui_media_playback(false, 2000, 1.5f, error, sizeof error));
    assert([[MPNowPlayingInfoCenter defaultCenter].nowPlayingInfo[MPNowPlayingInfoPropertyPlaybackRate] doubleValue] == 0);
    assert(!gpui_media_playback(true, 0, NAN, error, sizeof error));
    assert(gpui_media_release(error, sizeof error));
    assert([MPNowPlayingInfoCenter defaultCenter].nowPlayingInfo == nil);
    assert(!gpui_media_playback(true, 0, 1, error, sizeof error));
    assert(gpui_media_release(error, sizeof error));
}

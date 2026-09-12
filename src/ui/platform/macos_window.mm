#include <iostream>
#include <istream>
#include <ostream>
#include <random>
#include <vector>
#include <string>
#include <algorithm>
#undef timeout
#undef check
#include <mutex>
#include <shared_mutex>
#include <condition_variable>
#include <memory>
#import <Cocoa/Cocoa.h>
#import <MetalKit/MetalKit.h>
#include "../../AuraUltimate.hpp"
#include "../main/app_view.hpp"
#include "../../core/driver/mac_audio_driver.hpp"

@interface AuraAppDelegate : NSObject <NSApplicationDelegate, MTKViewDelegate, NSWindowDelegate> {
    std::unique_ptr<::Aura::Core::Driver::MacAudioDriver> _audioDriver;
}
@property (strong) NSWindow *window;
@property (strong) MTKView *metalView;
@property (strong) id eventMonitor;
@property (assign) BOOL didShutdown;
@end

@implementation AuraAppDelegate

- (void)applicationDidFinishLaunching:(NSNotification *)aNotification {
    self.didShutdown = NO;
    NSRect frame = NSMakeRect(0, 0, 1280, 800);
    NSUInteger style = NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskResizable | NSWindowStyleMaskMiniaturizable;
    
    _window = [[NSWindow alloc] initWithContentRect:frame
                                             styleMask:style
                                               backing:NSBackingStoreBuffered
                                                 defer:NO];
    [_window setTitle:@"Aura"];
    [_window setDelegate:self];
    [_window setBackgroundColor:[NSColor blackColor]];
    
    // Setup Metal View
    id<MTLDevice> device = MTLCreateSystemDefaultDevice();
    _metalView = [[MTKView alloc] initWithFrame:frame device:device];
    [_metalView setDelegate:self];
    [_metalView setPaused:NO];
    [_metalView setEnableSetNeedsDisplay:NO];
    
    [_window setContentView:_metalView];
    [_window makeKeyAndOrderFront:nil];
    [_window center];
    [NSApp activateIgnoringOtherApps:YES];

    // BOOT ENGINE
    auto& engine = ::Aura::AuraEngine::getInstance();
    const double sampleRate = 44100.0;
    const uint32_t blockSize = 512;
    engine.prepareToPlay(sampleRate, blockSize);

    // BOOT AUDIO
    auto audioDriver = std::make_unique<::Aura::Core::Driver::MacAudioDriver>([&engine](float* l, float* r, uint32_t len) {
        engine.process(l, r, len);
    });
    const bool audioReady = audioDriver->start(sampleRate, blockSize);
    if (!audioReady) {
    [_window setTitle:@"Aura — Audio Device Unavailable"];
        NSAlert *alert = [[NSAlert alloc] init];
        [alert setAlertStyle:NSAlertStyleWarning];
        [alert setMessageText:@"Audio device unavailable"];
    [alert setInformativeText:@"Aura opened without audio I/O. Check the selected device and try again."];
        [alert addButtonWithTitle:@"OK"];
        [alert runModal];
    } else {
        _audioDriver = std::move(audioDriver);
    }

    auto& appView = ::Aura::UI::Main::AuraAppView::getInstance();
    appView.bootstrap((__bridge void*)_metalView, _metalView.bounds.size.width, _metalView.bounds.size.height);
    appView.setScale(_metalView.layer.contentsScale);

    // MOUSE HANDLING INJECTION
    NSEventMask mask = NSEventMaskLeftMouseDown | NSEventMaskLeftMouseDragged | NSEventMaskLeftMouseUp | NSEventMaskMouseMoved | NSEventMaskKeyDown;
    self.eventMonitor = [NSEvent addLocalMonitorForEventsMatchingMask:mask handler:^NSEvent * _Nullable(NSEvent * _Nonnull event) {
        auto& view = ::Aura::UI::Main::AuraAppView::getInstance();
        
        if (event.type == NSEventTypeKeyDown) {
            // Preserve system-reserved shortcuts (Control/Option and
            // Command+Q/W) for AppKit; consume only DAW-owned keys.
            const NSEventModifierFlags reserved = NSEventModifierFlagControl | NSEventModifierFlagOption;
            if ((event.modifierFlags & reserved) != 0 ||
                ((event.modifierFlags & NSEventModifierFlagCommand) != 0 &&
                 (event.keyCode == 12 || event.keyCode == 13))) {
                return event;
            }
            bool cmd = (event.modifierFlags & NSEventModifierFlagCommand) != 0;
            bool shift = (event.modifierFlags & NSEventModifierFlagShift) != 0;
            view.handleKeyDown(event.keyCode, cmd, shift);
            return nil; // Consume key events for the DAW
        }

        // Keep input in AppKit points.  drawableSize/contentsScale are physical
        // pixels and must not be mixed into UI hit testing coordinates.
        const NSRect bounds = _metalView.bounds;
        NSPoint p = [_metalView convertPoint:[event locationInWindow] fromView:nil];
        const CGFloat logicalY = _metalView.isFlipped ? p.y : bounds.size.height - p.y;
        const float fx = std::clamp(static_cast<float>(p.x), 0.0f,
                                    static_cast<float>(std::max<CGFloat>(0.0, bounds.size.width)));
        const float fy = std::clamp(static_cast<float>(logicalY), 0.0f,
                                    static_cast<float>(std::max<CGFloat>(0.0, bounds.size.height)));
        
        if (event.type == NSEventTypeLeftMouseDown) view.handleMouseDown(fx, fy);
        else if (event.type == NSEventTypeLeftMouseDragged) view.handleMouseDrag(fx, fy);
        else if (event.type == NSEventTypeLeftMouseUp) view.handleMouseUp(fx, fy);
        
        return event;
    }];

    std::cout << "[macOS] GUI Host launched with HiDPI Scale: " << _metalView.layer.contentsScale << std::endl;
}

- (void)drawInMTKView:(MTKView *)view {
    (void)view;
    ::Aura::UI::Main::AuraAppView::getInstance().updateUI();
}

- (void)mtkView:(MTKView *)view drawableSizeWillChange:(CGSize)size {
    float bw = view.bounds.size.width;
    float bh = view.bounds.size.height;
    auto& appView = ::Aura::UI::Main::AuraAppView::getInstance();
    appView.onResize(bw, bh);
    appView.setScale(view.layer.contentsScale);
}

- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)sender {
    return YES;
}

- (void)windowWillClose:(NSNotification *)notification {
    (void)notification;
    [self shutdownAudioAndEvents];
}

- (void)shutdownAudioAndEvents {
    if (self.didShutdown) return;
    self.didShutdown = YES;
    if (self.eventMonitor) {
        [NSEvent removeMonitor:self.eventMonitor];
        self.eventMonitor = nil;
    }
    _audioDriver.reset();
    ::Aura::UI::Main::AuraAppView::getInstance().shutdown();
}

- (void)applicationWillTerminate:(NSNotification *)notification {
    (void)notification;
    [self shutdownAudioAndEvents];
}

@end

int main(int argc, const char * argv[]) {
    @autoreleasepool {
        NSApplication *app = [NSApplication sharedApplication];
        AuraAppDelegate *delegate = [[AuraAppDelegate alloc] init];
        [app setDelegate:delegate];
        [app setActivationPolicy:NSApplicationActivationPolicyRegular];
        [app run];
    }
    return 0;
}

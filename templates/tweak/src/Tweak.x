// logos: %hook, %orig, %log, %new and %ctor are turned into plain objective-c by `meow logos`
#import <UIKit/UIKit.h>

%hook SpringBoard

- (void)applicationDidFinishLaunching:(id)application {
	%log;
	%orig;
}

%end

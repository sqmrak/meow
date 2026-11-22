#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>

// comment with %hook inside
/* block comment with %end */

%hook SpringBoard

- (void)applicationDidFinishLaunching:(id)application {
	%orig;
	NSLog(@"launched %@", application);
}

- (BOOL)canShowAlerts { return YES; }

+ (id)sharedInstance {
	return %orig;
}

- (void)setFoo:(int)foo bar:(NSString *)bar baz:(CGRect)baz {
	%log;
	%log(@"extra", (int)foo);
	%orig(foo + 1, bar, baz);
}

%new
- (void)helloWorld {
	NSLog(@"hello");
}

%new(v@:i)
- (void)withEncoding:(int)x {
}

%new
- (int)computed:(float)f flag:(BOOL)b name:(NSString *)n {
	return 1;
}

%end

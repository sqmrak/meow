#import <Foundation/Foundation.h>

#if 0
%hook Dead
- (void)x { %orig; }
%end
#endif

%hook Live
- (void)y {
	// %orig; in a comment
	NSString *s = @"%orig // not a comment";
	%orig;
}
%end

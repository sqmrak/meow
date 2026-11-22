#import <Foundation/Foundation.h>

%hook A
- (void)x {
	%orig;
}
%end
#import <Foundation/Foundation.h>

%hook A
- (int)foo:(int)x {
	int (*f)(id, SEL, int) = (int (*)(id, SEL, int))&%orig;
	return f(self, _cmd, x);
}
%end

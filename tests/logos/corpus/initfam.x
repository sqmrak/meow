#import <UIKit/UIKit.h>

%hook A
- (id)init {
	return %orig;
}
- (instancetype)initWithFrame:(CGRect)f {
	return %orig;
}
- (id)copyWithZone:(NSZone *)z {
	return %orig;
}
+ (id)alloc {
	return %orig;
}
+ (id)new {
	return %orig;
}
- (void)dealloc {
	%orig;
}
- (void (^)(int))blockReturning:(void (^)(int x))cb {
	return %orig;
}
- (int (*)(int))fp:(int (*)(int, int))f other:(id<NSObject>)o {
	return %orig;
}
%end

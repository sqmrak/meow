#import <UIKit/UIKit.h>

%subclass MyView : UIView <NSCopying>
- (void)drawRect:(CGRect)rect {
	%orig;
}
+ (id)make {
	return [[%c(MyView) alloc] init];
}
%new
- (id)copyWithZone:(NSZone *)zone {
	return self;
}
%end

%subclass Plain : NSObject
- (id)init {
	self = %orig;
	return self;
}
%end

%ctor {
	%init;
}

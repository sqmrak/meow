#import <Foundation/Foundation.h>

%hook Hidden
- (void)foo { %orig; }
+ (void)bar { %orig; }
%end

%ctor {
	%init(Hidden = objc_getClass("RealHidden"));
}

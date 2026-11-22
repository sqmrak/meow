#import <UIKit/UIKit.h>

%hook UIView
- (void)layoutSubviews {
	%orig;
	[self setAlpha:0.5];
}
%end

%hook UILabel
- (NSString *)text {
	NSString *t = %orig;
	return [t stringByAppendingString:@"!"];
}
+ (Class)layerClass { return %orig; }
%end

%hook UIView
- (void)didMoveToSuperview {
	%orig;
}
%end

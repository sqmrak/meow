#import <UIKit/UIKit.h>

%group Old
%hook UIView
- (void)setHidden:(BOOL)hidden {
	%orig;
}
%end
%end

%group New
%hook UIView
- (void)setHidden:(BOOL)hidden {
	%orig(!hidden);
}
%end
%end

%hook UILabel
- (void)drawRect:(CGRect)r {
	%orig;
}
%end

%ctor {
	%init;
	if (1) {
		%init(Old);
	} else {
		%init(New);
	}
}

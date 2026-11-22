#import <UIKit/UIKit.h>

%hook A
- (void)foo {
	id x = [[%c(NSString) alloc] init];
	Class k = %c(+NSString);
	NSLog(@"%@ %@ %%d", x, k);
}
%end

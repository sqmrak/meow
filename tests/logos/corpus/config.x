#import <Foundation/Foundation.h>
%config(generator=internal)

%hook A
- (void)foo:(int)x { %orig; }
+ (void)bar { %orig; }
%new
- (void)baz { }
%end

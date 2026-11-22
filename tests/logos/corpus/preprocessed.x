# 1 "orig.x"
# 1 "<built-in>"
# 1 "<command-line>"
# 1 "orig.x"
#import <Foundation/Foundation.h>

%hook A
- (void)x { %orig; }
%end

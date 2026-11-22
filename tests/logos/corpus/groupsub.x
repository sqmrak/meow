#import <Foundation/Foundation.h>

%group G
%subclass S : NSObject
- (void)z { %orig; }
%end
%hookf(int, close, int fd) { return %orig; }
%hookf(int, dup, int fd) { return %orig(fd); }
%end

%ctor { %init(G); }

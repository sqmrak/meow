#import <Foundation/Foundation.h>
#if 0
#ifdef FOO
%hook X
#endif
%end
#endif
%hook A
- (void)x { %orig; }
%end

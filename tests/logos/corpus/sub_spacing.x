#import <Foundation/Foundation.h>
%subclass A:B<C>
%end
%subclass D :E
- (void)x { %orig; }
%end
%ctor { %init; }

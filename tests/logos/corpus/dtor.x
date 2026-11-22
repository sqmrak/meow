#import <Foundation/Foundation.h>

%hook A
- (void)foo { %orig; }
%end

%ctor {
	%init;
}

%dtor {
	NSLog(@"bye");
}

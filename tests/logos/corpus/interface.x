#import <Foundation/Foundation.h>
@protocol P;
@protocol Q <NSObject>
- (void)q;
@end
@interface Foo : NSObject {
	int x;
}
- (void)foo;
@end
@implementation Foo
- (void)foo {
	NSLog(@"%@", %c(Bar));
}
@end

%hook Bar
- (void)b { %orig; }
%end

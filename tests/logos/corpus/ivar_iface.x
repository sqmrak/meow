@interface Foo : NSObject
@end
%hook Foo
- (void)x { %orig; }
%end
@interface Bar : NSObject
@end

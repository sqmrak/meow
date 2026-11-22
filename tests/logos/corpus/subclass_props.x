#import <Foundation/Foundation.h>
%subclass S : NSObject
%property (nonatomic, retain) NSString *name;
- (id)init { return %orig; }
%end
%ctor { %init; }

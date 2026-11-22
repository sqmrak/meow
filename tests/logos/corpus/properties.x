#import <UIKit/UIKit.h>

%hook NSObject
%property (nonatomic, retain) NSString *tag;
%property (nonatomic, assign) int count;
%property (nonatomic, copy) NSArray *items;
%property (nonatomic, readonly) NSString *name;
%property (nonatomic, assign, getter=isReady, setter=setReady:) BOOL ready;
%property (retain) id thing;
%end

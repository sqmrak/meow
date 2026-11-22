#import <Foundation/Foundation.h>

@interface Sample : NSObject

@property (nonatomic, copy) NSString *title;
@property (nonatomic, readonly) NSUInteger count;
@property (nonatomic, assign, getter=isEnabled, setter=setEnabledState:) BOOL enabled;
@property (nonatomic, assign) CGRect frame;
@property (nonatomic, retain) id<NSObject> delegate;

- (instancetype)initWithTitle:(NSString *)title count:(NSUInteger)count;
- (void)reset;
- (BOOL)contains:(id)item;
- (NSRange)rangeOfItem:(id)item options:(int)options;
+ (instancetype)sharedSample;
- (CGPoint)center;
- (void)reset;

@end

@interface Sample (Extras)
- (NSString *)extra:(char)c;
@end

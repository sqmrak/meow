#import <Foundation/Foundation.h>

%hook A
- (void)first:(int)a
       second:(id)b
        third:(NSString *)c {
	%orig(a,
		b,
		c);
}
- (id)other {
	return [NSString stringWithFormat:@"x%@", %orig];
}
%end

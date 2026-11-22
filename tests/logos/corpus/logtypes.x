#import <Foundation/Foundation.h>

%hook A
- (void)a:(char)c b:(unsigned char)uc c:(short)s d:(long long)ll e:(unsigned long)ul f:(float)f g:(double)d h:(BOOL)b i:(SEL)sel j:(Class)k {
	%log;
}
- (void)p:(CGPoint)p s:(CGSize)s r:(CGRect)r n:(NSRange)n i:(NSInteger)i u:(NSUInteger)u {
	%log;
}
- (void)q:(char *)c v:(void *)v o:(NSString *)o a:(NSArray *)a t:(CGContextRef)ctx m:(MyStruct)m {
	%log;
}
- (void)blk:(void (^)(void))b {
	%log;
}
- (NSString *)ret {
	%log;
	NSString *r = %orig;
	return r;
}
%end

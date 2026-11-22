%hook A
- (CGRect)frame:(CGRect)r point:(CGPoint)p {
	%log;
	return %orig;
}
- (struct foo)s:(struct foo)f { return %orig; }
- (const char *)c:(const char *)c { return %orig; }
- (unsigned int)u { return %orig; }
- (void)arr:(int[3])a { }
%end

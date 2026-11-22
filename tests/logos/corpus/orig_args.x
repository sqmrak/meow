%hook A
- (int)x:(int)a y:(id)b {
	int r = %orig(a + 1, b) + %orig();
	foo(%orig(a, [b description]), 2);
	return r;
}
- (void)v:(int)a, ... {
	%orig(a);
}
- (id)z {
	return [%orig description];
}
- (void)w:(void (^)(void))blk {
	%orig(^{ NSLog(@"x"); });
}
%end

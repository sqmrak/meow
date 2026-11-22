%hook A
- (void)foo:(int)x {
	int y = x - (3);
	int z = x -(y) z;
	NSLog(@"%d", y -(x)w);
	%orig;
}
%end

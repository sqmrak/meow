%hook A
- (void)foo:(int)x {
	%log((int)x, (NSString *)@"y", (char *)"z");
}
%end

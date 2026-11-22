%hook A
- (void)log:(NSString *)fmt, ... {
	%orig(fmt);
}
%end

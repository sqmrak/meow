%hook A
- (void)x {
	NSString *a = @"http://example.com/*";
	NSString *b = @"/* not */";
	%orig;
}
%end

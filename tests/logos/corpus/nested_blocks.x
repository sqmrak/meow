%hook A
- (void)foo {
	dispatch_async(q, ^{
		if (x) {
			%orig;
		}
	});
}
%end

%hook A
- (void)x {
	NSLog(@"%hook %end %orig \" - (void)y");
	char c = '%';
	%orig;
}
%end

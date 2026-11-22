%hook A
- (void)x { %orig; }
%end
%ctor {
	%init;
	%init;
}

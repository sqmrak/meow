%hook A
- (void)x:(id)a {
	%orig(@"has ) paren", a);
	%orig(a, @")");
}
%end

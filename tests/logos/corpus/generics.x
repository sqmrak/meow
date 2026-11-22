%hook A
- (void)foo:(NSDictionary<NSString *, id> *)d bar:(NSArray<NSNumber *> *)a {
	%log;
	%orig;
}
%end

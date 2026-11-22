%class NSString
%hook A
- (void)x { [%c(NSString) new]; }
%end

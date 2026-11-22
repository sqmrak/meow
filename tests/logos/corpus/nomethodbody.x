%hook A
- (void)declOnly;
- (void)real { %orig; }
%end

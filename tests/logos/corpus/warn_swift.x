%hook Mod.Thing
- (void)x { %orig; }
%end
%ctor { %init; }

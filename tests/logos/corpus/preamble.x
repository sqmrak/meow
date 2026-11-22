#include <logos/logos.h>
#include <substrate.h>

%hook A
- (void)x { %orig; }
%end

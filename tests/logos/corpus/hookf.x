#import <stdio.h>
#include <fcntl.h>

%hookf(int, open, const char *path, int flags, ...) {
	return %orig(path, flags);
}

%hookf(void, callback, void (*cb)(int), int arr[10]) {
	%orig;
}

%hookf(int, noArgs, void) {
	return %orig();
}

%ctor {
	%init;
}

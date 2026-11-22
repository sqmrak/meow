#import <UIKit/UIKit.h>
#import "@PREFIX@AppDelegate.h"

int main(int argc, char *argv[]) {
	NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
	int rc = UIApplicationMain(argc, argv, nil, NSStringFromClass([@PREFIX@AppDelegate class]));
	[pool release];
	return rc;
}

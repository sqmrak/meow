%hookf(void, free, void *p) { %orig(p); }
%hookf(char *, getenv, const char *name) {
	char *(*f)(const char *) = &%orig;
	return f(name);
}
%ctor { %init; }

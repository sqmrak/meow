#include <stdint.h>
#include <stdio.h>

int32_t @PREFIX@_add(int32_t a, int32_t b);

int main(void) {
	printf("40 + 2 = %d\n", (int)@PREFIX@_add(40, 2));
	return 0;
}

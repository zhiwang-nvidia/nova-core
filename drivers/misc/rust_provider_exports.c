#include <linux/module.h>

struct add_args {
	int a;
	int b;
};

extern int rust_add_struct(const struct add_args *args);
EXPORT_SYMBOL(rust_add_struct);
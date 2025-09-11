#include <linux/init.h>
#include <linux/module.h>
#include <linux/kernel.h>

struct add_args {
	int a;
	int b;
};

extern int rust_add_struct(const struct add_args *args);

static int __init c_consumer_init(void)
{
	struct add_args args = { .a = 5, .b = 7 };
	pr_info("C consumer: rust_add_struct({5, 7}) = %d\n", rust_add_struct(&args));
	return 0;
}

static void __exit c_consumer_exit(void)
{
}

module_init(c_consumer_init);
module_exit(c_consumer_exit);
MODULE_LICENSE("GPL");
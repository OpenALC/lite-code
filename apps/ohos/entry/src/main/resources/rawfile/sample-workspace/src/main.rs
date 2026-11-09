mod util;

use util::constants::APP_NAME;
use util::message::greeting;

fn main() {
    // 示例入口：验证文件树与只读预览能展示嵌套目录中的真实文件。
    println!("{} 示例程序已启动", APP_NAME);
    println!("{}", greeting("鸿蒙"));
}

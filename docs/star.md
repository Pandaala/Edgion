
我们解决了哪些问题：
1、所有用户配置的动态加载 Route/Backend/Filter(plugins)/TlsCert等全部都是随着yaml文件修改立即生效的。
2、同时兼顾k8s部署和物理机器部署，可以一套网关通吃
3、直接使用k8s gateway api, 最大范围兼容，配置友好，ai友好
4、默认使用jemalloc，大部分开放的nginx/openresty使用默认libc，无法及时定位问题和分析调用栈
5、合并了nginx里的access log和error log， 一条acess log反应出所有细节。

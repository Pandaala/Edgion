# RegionRoute 插件

请参阅 [RegionRoute 配置参考](../http-route/filters/edgion-plugins/region-route.md)，了解 ClusterRegionSetting、必填引用、路由规则及异常策略。

## Center 浏览与操作

进入 Controller 后，资源页面通过后端分页和计数读取 Kubernetes、文件或 etcd
中的源配置，默认每页 10 条。Center 的 Cluster/Region 目录使用专用轻量
List/Watch 索引，只包含设置身份和 RegionRoute 插件引用，不镜像完整区域表、
Gateway 加工后资源或凭据。Region 归属仅来自 ClusterRegionSetting.regionName，
并保留 Controller 和源资源身份。

点击目录后重新读取详情。切换故障转移目标需要先预览，再对单个目标提交条件写入；
源内容或目录身份变化会使预览失效。发生冲突后必须刷新并重新预览。写入成功只代表
配置已持久化，不代表 Controller 已观察到变化或 Gateway 已应用。可靠多目标操作
仍属于单独的后续范围。

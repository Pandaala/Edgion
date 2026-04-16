# Phase 4：前端 RegionRoute 页面重建

## 目标

删除现有 Center RegionRoute 页面（`ClusterRegionRoutePage.tsx`、`RegionRoutePage.tsx`），重新基于 Phase 3 的新 API 构建。同一页面组件支持 Controller 和 Center 模式。

## 需删除的文件

- `src/pages/Center/ClusterRegionRoutePage.tsx`
- `src/pages/Center/RegionRoutePage.tsx`
- `src/api/center.ts` 中 RegionRoute 相关的类型和方法（保留 Controller 管理相关方法）

## 新页面结构

### ClusterRegionRoute 表格

| 列 | 数据来源 | 说明 |
|---|---|---|
| Namespace | key 解析 | PM 的 namespace |
| Name | key 解析 | PM 的 name |
| Controllers | Center 模式独有 | 哪些 controller 有这个配置 |
| Regions | PM 数据 | 区域列表摘要 |
| 操作 | | 详情/Failover |

key = `"namespace/name"`

### ServiceRegionRoute 表格

| 列 | 数据来源 | 说明 |
|---|---|---|
| Cluster Ref (ns/name) | key 解析 | 引用的 ClusterRegionRoute |
| Namespace | key 解析 | Service PM 的 namespace |
| Name | key 解析 | Service PM 的 name |
| Controllers | Center 模式独有 | |
| Regions | PM 数据 | failover 覆盖摘要 |
| 操作 | | 详情/Failover |

key = `"ref_ns/ref_name/namespace/name"`

### 页面路由

```
Center 模式：
  /region-routes/cluster     → ClusterRegionRouteList
  /region-routes/service     → ServiceRegionRouteList

Controller 模式（透明代理内，同一页面组件）：
  /region-routes/cluster     → ClusterRegionRouteList
  /region-routes/service     → ServiceRegionRouteList
```

## 新 API 层

```typescript
// src/api/regionRoute.ts

// --- 类型 ---

interface ClusterRegionRouteEntry {
  namespace: string
  name: string
  controllers: Record<string, any>  // controller_id → PM 配置（Center 模式）
  // Controller 模式下只有一个 key，或直接返回 PM 值
}

interface ServiceRegionRouteEntry {
  clusterRefNamespace: string
  clusterRefName: string
  namespace: string
  name: string
  controllers: Record<string, any>
}

// --- API 方法 ---

// 自动根据 mode 选择 endpoint
export const regionRouteApi = {
  listClusterRegionRoutes: () => GET(centerOrController('/cluster-region-routes')),
  listServiceRegionRoutes: () => GET(centerOrController('/service-region-routes')),
  // failover 保留（Phase 5 AA 操作）
}
```

## 步骤

### 4.1 新建 API 层

- `src/api/regionRoute.ts`：类型定义 + API 方法
- 根据 app mode 自动切换 Controller/Center endpoint

### 4.2 新建页面组件

- `src/pages/RegionRoute/ClusterRegionRouteList.tsx`
- `src/pages/RegionRoute/ServiceRegionRouteList.tsx`
- 使用 Ant Design Table，React Query 获取数据
- Center 模式：展开行显示各 controller 的配置详情
- Controller 模式：直接显示本地配置
- 所有 UI 文字走 i18n

### 4.3 更新路由

- `src/App.tsx`：更新路由注册
- `src/components/Layout/CenterLayout.tsx`：侧边栏导航已有（Phase 0 已改 Region→Cluster）

### 4.4 删除旧页面

- 删除 `src/pages/Center/ClusterRegionRoutePage.tsx`
- 删除 `src/pages/Center/RegionRoutePage.tsx`
- 清理 `src/api/center.ts` 中的 RegionRoute 相关代码

### 4.5 i18n

- `src/i18n/en.ts` 和 `src/i18n/zh.ts` 同步更新
- 新增 RegionRoute 页面相关 key
- 清理旧 key

### 4.6 测试

- Center 模式：启动 Center + Controller pods，访问 http://localhost:5174
- Controller 模式：直连 Controller，验证同一页面组件正常显示
- 检查表格渲染、展开行、排序、搜索

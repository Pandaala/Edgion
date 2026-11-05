# RegionRoute Plugin

See the [RegionRoute configuration reference](../http-route/filters/edgion-plugins/region-route.md) for ClusterRegionSetting, required references, routing rules and failure policy.

## Center browsing and operations

Inside a Controller, resource pages read Kubernetes, filesystem or etcd source
configuration through backend pagination and counts (ten items by default).
The Center Cluster/Region directory uses a dedicated lightweight List/Watch index
of settings and RegionRoute plugin references. It does not mirror full routing
tables, processed Gateway resources or credentials. Region membership comes only
from ClusterRegionSetting.regionName, with Controller/source provenance retained.

Opening a row reads fresh detail. A failover edit requires a fresh single-target
preview and conditional apply; source or directory identity changes invalidate
the preview. A conflict requires refresh and another preview. Successful writes
prove persistence only, not Controller observation or Gateway application.
Reliable multi-target orchestration remains a separate future scope.

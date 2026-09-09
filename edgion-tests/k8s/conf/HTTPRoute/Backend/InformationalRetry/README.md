# Informational response timeout retry coverage

This Kubernetes and local-stack suite routes one HTTP/1.1 request through the Gateway to the raw transport backend. The first trace-keyed upstream attempt flushes `100 Continue` and then stalls past the one-second backend-request timeout. The second attempt returns the bounded echo response. The client and recorder jointly prove that the informational head did not suppress retry and that the downstream exchange contains one informational head followed by one intact final response.

// A migration sample, not a third-party plugin compatibility certificate.
package main

import (
	"github.com/tetratelabs/proxy-wasm-go-sdk/proxywasm"
	"github.com/tetratelabs/proxy-wasm-go-sdk/proxywasm/types"
)

func main() {}
func init() { proxywasm.SetVMContext(&vm{}) }

type vm struct{ types.DefaultVMContext }

func (*vm) NewPluginContext(uint32) types.PluginContext { return &plugin{} }

type plugin struct{ types.DefaultPluginContext }

func (*plugin) NewHttpContext(uint32) types.HttpContext { return &request{} }

type request struct{ types.DefaultHttpContext }

func (*request) OnHttpRequestHeaders(int, bool) types.Action {
	value, err := proxywasm.GetHttpRequestHeader("x-policy-deny")
	if err == nil && value == "1" {
		_ = proxywasm.SendHttpResponse(403, nil, []byte("policy denied"), -1)
		return types.ActionPause
	}
	_ = proxywasm.ReplaceHttpRequestHeader("x-policy-checked", "yes")
	return types.ActionContinue
}

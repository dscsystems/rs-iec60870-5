// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.
//
// go101cli is an IEC 60870-5-101 primary station built on go-iecp5, carried
// over the TCP encapsulation transport.
//
// Usage: go101cli <host:port>
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/riclolsen/go-iecp5/asdu"
	"github.com/riclolsen/go-iecp5/cs101"
)

var out = struct {
	sync.Mutex
	enc *json.Encoder
}{enc: json.NewEncoder(os.Stdout)}

func emit(event map[string]interface{}) {
	out.Lock()
	defer out.Unlock()
	_ = out.enc.Encode(event)
	_ = os.Stdout.Sync()
}

type handler struct{}

// report decodes an ASDU's information objects into JSON events.
func report(pack *asdu.ASDU, kind string) {
	typ := strings.Trim(pack.Type.String(), "TID<>")
	switch pack.Type {
	case asdu.M_SP_NA_1, asdu.M_SP_TA_1, asdu.M_SP_TB_1:
		for _, p := range pack.GetSinglePoint() {
			ev := map[string]interface{}{
				"event": "single_point", "via": kind, "type": typ, "ioa": uint(p.Ioa),
				"value": p.Value, "qds": byte(p.Qds), "cause": byte(pack.Coa.Cause),
			}
			if !p.Time.IsZero() {
				ev["time"] = p.Time.UTC().Format(time.RFC3339Nano)
			}
			emit(ev)
		}
	case asdu.M_DP_NA_1, asdu.M_DP_TA_1, asdu.M_DP_TB_1:
		for _, p := range pack.GetDoublePoint() {
			emit(map[string]interface{}{
				"event": "double_point", "via": kind, "type": typ,
				"ioa": uint(p.Ioa), "value": byte(p.Value),
			})
		}
	case asdu.M_ME_NB_1, asdu.M_ME_TB_1, asdu.M_ME_TE_1:
		for _, p := range pack.GetMeasuredValueScaled() {
			emit(map[string]interface{}{
				"event": "measured_scaled", "via": kind, "type": typ,
				"ioa": uint(p.Ioa), "value": p.Value,
			})
		}
	case asdu.M_ME_NC_1, asdu.M_ME_TC_1, asdu.M_ME_TF_1:
		for _, p := range pack.GetMeasuredValueFloat() {
			emit(map[string]interface{}{
				"event": "measured_float", "via": kind, "type": typ, "ioa": uint(p.Ioa),
				"value": p.Value, "qds": byte(p.Qds), "cause": byte(pack.Coa.Cause),
			})
		}
	case asdu.M_IT_NA_1, asdu.M_IT_TA_1, asdu.M_IT_TB_1:
		for _, p := range pack.GetIntegratedTotals() {
			emit(map[string]interface{}{
				"event": "integrated_total", "via": kind, "type": typ, "ioa": uint(p.Ioa),
				"count": p.Value.CounterReading, "cause": byte(pack.Coa.Cause),
			})
		}
	default:
		emit(map[string]interface{}{
			"event": "asdu", "via": kind, "type": typ, "cause": byte(pack.Coa.Cause),
		})
	}
}

// InterrogationHandler receives interrogation-caused data on the cs101 client.
func (handler) InterrogationHandler(pack *asdu.ASDU) error {
	report(pack, "interrogation")
	return nil
}

func (handler) CounterInterrogationHandler(pack *asdu.ASDU) error {
	report(pack, "counter")
	return nil
}

func (handler) ReadHandler(pack *asdu.ASDU) error {
	report(pack, "read")
	return nil
}

func (handler) TestCommandHandler(pack *asdu.ASDU) error {
	report(pack, "test")
	return nil
}

func (handler) ClockSyncHandler(pack *asdu.ASDU) error {
	report(pack, "clock")
	return nil
}

func (handler) ResetProcessHandler(pack *asdu.ASDU) error {
	report(pack, "reset")
	return nil
}

func (handler) DelayAcquisitionHandler(pack *asdu.ASDU) error {
	report(pack, "delay")
	return nil
}

// ASDUHandler receives spontaneous data and command confirmations.
func (handler) ASDUHandler(pack *asdu.ASDU, _ int) error {
	report(pack, "asdu")
	return nil
}

func (handler) ASDUHandlerAll(_ *asdu.ASDU, _ int) error { return nil }

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: go101cli <host:port>")
		os.Exit(2)
	}

	cfg := cs101.DefaultConfig()
	cfg.Transport = cs101.TransportTCPClient
	cfg.TCP = cs101.TCPConfig{Address: os.Args[1]}
	cfg.LinkAddress = 1
	cfg.LinkAddrSize = 1
	cfg.TimeoutSendLinkMsg = 20 * time.Millisecond

	opt := cs101.NewOption()
	if err := opt.SetConfig(cfg); err != nil {
		fmt.Fprintln(os.Stderr, "bad config:", err)
		os.Exit(2)
	}
	_ = opt.SetParams(asdu.ParamsStandard101)

	cli := cs101.NewClient(handler{}, opt)
	if cli == nil {
		fmt.Fprintln(os.Stderr, "the client could not be created")
		os.Exit(1)
	}
	cli.SetLogMode(os.Getenv("IECP5_DEBUG") != "")

	if err := cli.Start(); err != nil {
		fmt.Fprintln(os.Stderr, "start failed:", err)
		os.Exit(1)
	}

	// Wait for the link procedure to complete before issuing anything.
	for i := 0; i < 200 && !cli.IsLinkActive(); i++ {
		time.Sleep(50 * time.Millisecond)
	}
	if !cli.IsLinkActive() {
		emit(map[string]interface{}{"event": "error", "error": "link never became active"})
		os.Exit(1)
	}
	emit(map[string]interface{}{"event": "link_active"})

	coa := asdu.CauseOfTransmission{Cause: asdu.Activation}
	if err := cli.InterrogationCmd(coa, 1, asdu.QOIStation); err != nil {
		emit(map[string]interface{}{"event": "error", "op": "interrogation", "error": err.Error()})
	}

	time.Sleep(500 * time.Millisecond)
	_ = asdu.SingleCmd(cli, asdu.C_SC_NA_1, coa, 1,
		asdu.SingleCommandInfo{Ioa: 6000, Value: true,
			Qoc: asdu.QualifierOfCommand{Qual: asdu.QOCShortPulseDuration}})

	time.Sleep(300 * time.Millisecond)
	_ = asdu.SetpointCmdFloat(cli, asdu.C_SE_NC_1, coa, 1,
		asdu.SetpointCommandFloatInfo{Ioa: 6002, Value: -12.75})

	time.Sleep(300 * time.Millisecond)
	_ = cli.ClockSynchronizationCmd(coa, 1, time.Now().UTC())

	time.Sleep(300 * time.Millisecond)
	emit(map[string]interface{}{"event": "done"})

	timeout := 30 * time.Second
	if v := os.Getenv("IECP5_TIMEOUT_SECS"); v != "" {
		var secs int
		if _, err := fmt.Sscanf(v, "%d", &secs); err == nil && secs > 0 {
			timeout = time.Duration(secs) * time.Second
		}
	}
	time.Sleep(timeout)
	_ = cli.Close()
}

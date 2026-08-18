// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.
//
// gocli is an IEC 60870-5-104 master built on go-iecp5, used to verify that
// this crate's controlled station interoperates with a known-good peer.
//
// Usage: gocli <host:port>
//
// It activates the connection, runs an interrogation, sends one command of each
// control family, and reports everything it receives as JSON lines on stdout.
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/riclolsen/go-iecp5/asdu"
	"github.com/riclolsen/go-iecp5/cs104"
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

// ASDUHandler receives all monitor-direction data, including the interrogation
// response, and reports each information object.
func (handler) ASDUHandler(_ asdu.Connect, pack *asdu.ASDU, _ *cs104.Server, _ int) error {
	typ := strings.Trim(pack.Type.String(), "TID<>")

	switch pack.Type {
	case asdu.M_SP_NA_1, asdu.M_SP_TA_1, asdu.M_SP_TB_1:
		for _, p := range pack.GetSinglePoint() {
			ev := map[string]interface{}{
				"event": "single_point", "type": typ, "ioa": uint(p.Ioa),
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
				"event": "double_point", "type": typ, "ioa": uint(p.Ioa),
				"value": byte(p.Value), "qds": byte(p.Qds),
			})
		}
	case asdu.M_ST_NA_1, asdu.M_ST_TA_1, asdu.M_ST_TB_1:
		for _, p := range pack.GetStepPosition() {
			emit(map[string]interface{}{
				"event": "step_position", "type": typ, "ioa": uint(p.Ioa),
				"value": p.Value.Val, "transient": p.Value.HasTransient,
			})
		}
	case asdu.M_BO_NA_1, asdu.M_BO_TA_1, asdu.M_BO_TB_1:
		for _, p := range pack.GetBitString32() {
			emit(map[string]interface{}{
				"event": "bitstring", "type": typ, "ioa": uint(p.Ioa), "value": p.Value,
			})
		}
	case asdu.M_ME_NA_1, asdu.M_ME_TA_1, asdu.M_ME_TD_1, asdu.M_ME_ND_1:
		for _, p := range pack.GetMeasuredValueNormal() {
			emit(map[string]interface{}{
				"event": "measured_normal", "type": typ, "ioa": uint(p.Ioa),
				"raw": int(p.Value), "value": p.Value.Float64(), "qds": byte(p.Qds),
			})
		}
	case asdu.M_ME_NB_1, asdu.M_ME_TB_1, asdu.M_ME_TE_1:
		for _, p := range pack.GetMeasuredValueScaled() {
			emit(map[string]interface{}{
				"event": "measured_scaled", "type": typ, "ioa": uint(p.Ioa),
				"value": p.Value, "qds": byte(p.Qds),
			})
		}
	case asdu.M_ME_NC_1, asdu.M_ME_TC_1, asdu.M_ME_TF_1:
		for _, p := range pack.GetMeasuredValueFloat() {
			ev := map[string]interface{}{
				"event": "measured_float", "type": typ, "ioa": uint(p.Ioa),
				"value": p.Value, "qds": byte(p.Qds), "cause": byte(pack.Coa.Cause),
				"sq": pack.Variable.IsSequence,
			}
			if !p.Time.IsZero() {
				ev["time"] = p.Time.UTC().Format(time.RFC3339Nano)
			}
			emit(ev)
		}
	case asdu.M_IT_NA_1, asdu.M_IT_TA_1, asdu.M_IT_TB_1:
		for _, p := range pack.GetIntegratedTotals() {
			emit(map[string]interface{}{
				"event": "integrated_total", "type": typ, "ioa": uint(p.Ioa),
				"count": p.Value.CounterReading, "seq": p.Value.SeqNumber,
				"carry": p.Value.HasCarry, "cause": byte(pack.Coa.Cause),
			})
		}
	case asdu.M_EI_NA_1:
		ioa, coi := pack.GetEndOfInitialization()
		emit(map[string]interface{}{
			"event": "end_of_init", "ioa": uint(ioa), "cause": byte(coi.Cause),
		})
	default:
		emit(map[string]interface{}{"event": "asdu", "type": typ, "cause": byte(pack.Coa.Cause)})
	}
	return nil
}

func (handler) ASDUHandlerAll(_ asdu.Connect, _ *asdu.ASDU, _ *cs104.Server, _ int) error {
	return nil
}

// The dedicated handlers receive the mirrored command confirmations.
func (handler) InterrogationHandler(_ asdu.Connect, pack *asdu.ASDU) error {
	emit(map[string]interface{}{
		"event": "interrogation_reply", "cause": byte(pack.Coa.Cause),
		"negative": pack.Coa.IsNegative,
	})
	return nil
}

func (handler) CounterInterrogationHandler(_ asdu.Connect, pack *asdu.ASDU) error {
	emit(map[string]interface{}{"event": "counter_reply", "cause": byte(pack.Coa.Cause)})
	return nil
}

func (handler) ReadHandler(_ asdu.Connect, pack *asdu.ASDU) error {
	emit(map[string]interface{}{"event": "read_reply", "cause": byte(pack.Coa.Cause)})
	return nil
}

func (handler) TestCommandHandler(_ asdu.Connect, pack *asdu.ASDU) error {
	emit(map[string]interface{}{"event": "test_reply", "cause": byte(pack.Coa.Cause)})
	return nil
}

func (handler) ClockSyncHandler(_ asdu.Connect, pack *asdu.ASDU) error {
	emit(map[string]interface{}{"event": "clock_sync_reply", "cause": byte(pack.Coa.Cause)})
	return nil
}

func (handler) ResetProcessHandler(_ asdu.Connect, pack *asdu.ASDU) error {
	emit(map[string]interface{}{"event": "reset_reply", "cause": byte(pack.Coa.Cause)})
	return nil
}

func (handler) DelayAcquisitionHandler(_ asdu.Connect, pack *asdu.ASDU) error {
	emit(map[string]interface{}{"event": "delay_reply", "cause": byte(pack.Coa.Cause)})
	return nil
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: gocli <host:port>")
		os.Exit(2)
	}

	option := cs104.NewOption()
	if err := option.AddRemoteServer(os.Args[1]); err != nil {
		fmt.Fprintln(os.Stderr, "bad address:", err)
		os.Exit(2)
	}

	client := cs104.NewClient(handler{}, option)
	if os.Getenv("IECP5_DEBUG") != "" {
		client.LogMode(true)
	}

	client.SetOnConnectHandler(func(c *cs104.Client) { c.SendStartDt() })
	client.SetOnActivatedHandler(func(c *cs104.Client) {
		emit(map[string]interface{}{"event": "activated"})

		coa := asdu.CauseOfTransmission{Cause: asdu.Activation}
		if err := c.InterrogationCmd(coa, 1, asdu.QOIStation); err != nil {
			emit(map[string]interface{}{"event": "error", "op": "interrogation", "error": err.Error()})
		}

		// One command of each control family, spaced so the outstation's
		// confirmations are not batched into a single window.
		go func() {
			time.Sleep(300 * time.Millisecond)
			_ = asdu.SingleCmd(c, asdu.C_SC_NA_1, coa, 1,
				asdu.SingleCommandInfo{Ioa: 6000, Value: true,
					Qoc: asdu.QualifierOfCommand{Qual: asdu.QOCShortPulseDuration}})

			time.Sleep(100 * time.Millisecond)
			_ = asdu.DoubleCmd(c, asdu.C_DC_NA_1, coa, 1,
				asdu.DoubleCommandInfo{Ioa: 6001, Value: asdu.DCOOn})

			time.Sleep(100 * time.Millisecond)
			_ = asdu.SetpointCmdFloat(c, asdu.C_SE_NC_1, coa, 1,
				asdu.SetpointCommandFloatInfo{Ioa: 6002, Value: -12.75})

			time.Sleep(100 * time.Millisecond)
			_ = asdu.BitsString32Cmd(c, asdu.C_BO_NA_1, coa, 1,
				asdu.BitsString32CommandInfo{Ioa: 6003, Value: 0x0f0f0f0f})

			time.Sleep(100 * time.Millisecond)
			_ = c.ClockSynchronizationCmd(coa, 1, time.Now().UTC())

			time.Sleep(100 * time.Millisecond)
			_ = c.CounterInterrogationCmd(coa, 1,
				asdu.QualifierCountCall{Request: asdu.QCCTotal, Freeze: asdu.QCCFrzRead})

			time.Sleep(100 * time.Millisecond)
			_ = c.TestCommand(coa, 1)

			time.Sleep(100 * time.Millisecond)
			emit(map[string]interface{}{"event": "done"})
		}()
	})

	if err := client.Start(); err != nil {
		fmt.Fprintln(os.Stderr, "start failed:", err)
		os.Exit(1)
	}

	// Run for a bounded time; the test kills us earlier once it has seen enough.
	timeout := 30 * time.Second
	if v := os.Getenv("IECP5_TIMEOUT_SECS"); v != "" {
		var secs int
		if _, err := fmt.Sscanf(v, "%d", &secs); err == nil && secs > 0 {
			timeout = time.Duration(secs) * time.Second
		}
	}
	time.Sleep(timeout)
	_ = client.Close()
}

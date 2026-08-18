// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.
//
// gosrv is an IEC 60870-5-104 controlled station built on go-iecp5, used to
// verify that this crate's master interoperates with a known-good peer.
//
// It binds an ephemeral port, prints it, then reports everything it receives
// as JSON lines on stdout so the Rust test can assert on the exchange.
package main

import (
	"encoding/json"
	"fmt"
	"net"
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

// emit writes one JSON event line and flushes it, so the test never blocks
// waiting on a buffer.
func emit(event map[string]interface{}) {
	out.Lock()
	defer out.Unlock()
	_ = out.enc.Encode(event)
	_ = os.Stdout.Sync()
}

type handler struct{}

// InterrogationHandler answers a station interrogation with a fixed process
// image covering every monitor-direction family the Rust side asserts on.
func (handler) InterrogationHandler(c asdu.Connect, pack *asdu.ASDU, qoi asdu.QualifierOfInterrogation) error {
	emit(map[string]interface{}{"event": "interrogation", "qoi": byte(qoi), "ca": uint(pack.CommonAddr)})

	if qoi != asdu.QOIStation {
		pack.Coa.IsNegative = true
		return pack.SendReplyMirror(c, asdu.ActivationCon)
	}
	if err := pack.SendReplyMirror(c, asdu.ActivationCon); err != nil {
		return err
	}

	coa := asdu.CauseOfTransmission{Cause: asdu.InterrogatedByStation}
	ca := pack.CommonAddr

	if err := asdu.Single(c, false, coa, ca,
		asdu.SinglePointInfo{Ioa: 100, Value: true, Qds: asdu.QDSGood},
		asdu.SinglePointInfo{Ioa: 101, Value: false, Qds: asdu.QDSInvalid},
	); err != nil {
		return err
	}
	if err := asdu.Double(c, false, coa, ca,
		asdu.DoublePointInfo{Ioa: 200, Value: asdu.DPIDeterminedOn, Qds: asdu.QDSGood},
	); err != nil {
		return err
	}
	if err := asdu.Step(c, false, coa, ca,
		asdu.StepPositionInfo{Ioa: 250, Value: asdu.StepPosition{Val: -17, HasTransient: true}},
	); err != nil {
		return err
	}
	if err := asdu.BitString32(c, false, coa, ca,
		asdu.BitString32Info{Ioa: 300, Value: 0xdeadbeef},
	); err != nil {
		return err
	}
	if err := asdu.MeasuredValueNormal(c, false, coa, ca,
		asdu.MeasuredValueNormalInfo{Ioa: 350, Value: asdu.Normalize(16384)},
	); err != nil {
		return err
	}
	if err := asdu.MeasuredValueScaled(c, false, coa, ca,
		asdu.MeasuredValueScaledInfo{Ioa: 380, Value: -1234},
	); err != nil {
		return err
	}
	if err := asdu.MeasuredValueFloat(c, false, coa, ca,
		asdu.MeasuredValueFloatInfo{Ioa: 400, Value: 22.5, Qds: asdu.QDSGood},
		asdu.MeasuredValueFloatInfo{Ioa: 401, Value: -1.25, Qds: asdu.QDSOverflow},
	); err != nil {
		return err
	}
	// A sequence (SQ=1): consecutive addresses, one address on the wire.
	if err := asdu.MeasuredValueFloat(c, true, coa, ca,
		asdu.MeasuredValueFloatInfo{Ioa: 450, Value: 1.0},
		asdu.MeasuredValueFloatInfo{Ioa: 451, Value: 2.0},
		asdu.MeasuredValueFloatInfo{Ioa: 452, Value: 3.0},
	); err != nil {
		return err
	}
	if err := asdu.IntegratedTotals(c, false,
		asdu.CauseOfTransmission{Cause: asdu.Spontaneous}, ca,
		asdu.BinaryCounterReadingInfo{Ioa: 500, Value: asdu.BinaryCounterReading{
			CounterReading: 4242, SeqNumber: 1, HasCarry: true,
		}},
	); err != nil {
		return err
	}
	// Time-tagged, to check CP56Time2a in both directions.
	if err := asdu.SingleCP56Time2a(c,
		asdu.CauseOfTransmission{Cause: asdu.Spontaneous}, ca,
		asdu.SinglePointInfo{Ioa: 600, Value: true, Time: time.Date(2026, 8, 17, 12, 34, 56, 789000000, time.UTC)},
	); err != nil {
		return err
	}

	return pack.SendReplyMirror(c, asdu.ActivationTerm)
}

func (handler) CounterInterrogationHandler(c asdu.Connect, pack *asdu.ASDU, qcc asdu.QualifierCountCall) error {
	emit(map[string]interface{}{"event": "counter_interrogation", "request": byte(qcc.Request), "freeze": byte(qcc.Freeze)})
	if err := pack.SendReplyMirror(c, asdu.ActivationCon); err != nil {
		return err
	}
	if err := asdu.IntegratedTotals(c, false,
		asdu.CauseOfTransmission{Cause: asdu.RequestByGeneralCounter}, pack.CommonAddr,
		asdu.BinaryCounterReadingInfo{Ioa: 501, Value: asdu.BinaryCounterReading{CounterReading: 99}},
	); err != nil {
		return err
	}
	return pack.SendReplyMirror(c, asdu.ActivationTerm)
}

func (handler) ReadHandler(c asdu.Connect, pack *asdu.ASDU, ioa asdu.InfoObjAddr) error {
	emit(map[string]interface{}{"event": "read", "ioa": uint(ioa)})
	return asdu.MeasuredValueFloat(c, false,
		asdu.CauseOfTransmission{Cause: asdu.Request}, pack.CommonAddr,
		asdu.MeasuredValueFloatInfo{Ioa: ioa, Value: 7.5})
}

func (handler) ClockSyncHandler(c asdu.Connect, pack *asdu.ASDU, t time.Time) error {
	emit(map[string]interface{}{"event": "clock_sync", "time": t.UTC().Format(time.RFC3339Nano)})
	return pack.SendReplyMirror(c, asdu.ActivationCon)
}

func (handler) ResetProcessHandler(c asdu.Connect, pack *asdu.ASDU, qrp asdu.QualifierOfResetProcessCmd) error {
	emit(map[string]interface{}{"event": "reset_process", "qrp": byte(qrp)})
	return pack.SendReplyMirror(c, asdu.ActivationCon)
}

func (handler) DelayAcquisitionHandler(c asdu.Connect, pack *asdu.ASDU, msec uint16) error {
	emit(map[string]interface{}{"event": "delay_acquisition", "msec": msec})
	return pack.SendReplyMirror(c, asdu.ActivationCon)
}

func (handler) ASDUHandlerAll(_ asdu.Connect, pack *asdu.ASDU, _ int) error {
	emit(map[string]interface{}{
		"event": "asdu",
		"type":  strings.Trim(pack.Type.String(), "TID<>"),
		"cause": byte(pack.Coa.Cause),
		"ca":    uint(pack.CommonAddr),
	})
	return nil
}

// ASDUHandler receives the control commands and confirms them.
func (handler) ASDUHandler(c asdu.Connect, pack *asdu.ASDU) error {
	switch pack.Type {
	case asdu.C_SC_NA_1, asdu.C_SC_TA_1:
		cmd := pack.GetSingleCmd()
		emit(map[string]interface{}{
			"event": "single_cmd", "ioa": uint(cmd.Ioa), "value": cmd.Value,
			"qual": byte(cmd.Qoc.Qual), "select": cmd.Qoc.InSelect,
		})
		if err := pack.SendReplyMirror(c, asdu.ActivationCon); err != nil {
			return err
		}
		return pack.SendReplyMirror(c, asdu.ActivationTerm)

	case asdu.C_DC_NA_1, asdu.C_DC_TA_1:
		cmd := pack.GetDoubleCmd()
		emit(map[string]interface{}{"event": "double_cmd", "ioa": uint(cmd.Ioa), "value": byte(cmd.Value)})
		if err := pack.SendReplyMirror(c, asdu.ActivationCon); err != nil {
			return err
		}
		return pack.SendReplyMirror(c, asdu.ActivationTerm)

	case asdu.C_RC_NA_1:
		cmd := pack.GetStepCmd()
		emit(map[string]interface{}{"event": "step_cmd", "ioa": uint(cmd.Ioa), "value": byte(cmd.Value)})
		return pack.SendReplyMirror(c, asdu.ActivationCon)

	case asdu.C_SE_NA_1:
		cmd := pack.GetSetpointNormalCmd()
		emit(map[string]interface{}{"event": "setpoint_normal", "ioa": uint(cmd.Ioa), "value": int(cmd.Value)})
		return pack.SendReplyMirror(c, asdu.ActivationCon)

	case asdu.C_SE_NB_1:
		cmd := pack.GetSetpointCmdScaled()
		emit(map[string]interface{}{"event": "setpoint_scaled", "ioa": uint(cmd.Ioa), "value": cmd.Value})
		return pack.SendReplyMirror(c, asdu.ActivationCon)

	case asdu.C_SE_NC_1:
		cmd := pack.GetSetpointFloatCmd()
		emit(map[string]interface{}{"event": "setpoint_float", "ioa": uint(cmd.Ioa), "value": cmd.Value})
		return pack.SendReplyMirror(c, asdu.ActivationCon)

	case asdu.C_BO_NA_1:
		cmd := pack.GetBitsString32Cmd()
		emit(map[string]interface{}{"event": "bitstring_cmd", "ioa": uint(cmd.Ioa), "value": cmd.Value})
		return pack.SendReplyMirror(c, asdu.ActivationCon)
	}
	// Unknown to this outstation: the session replies UnknownTypeID.
	return fmt.Errorf("unhandled type %v", pack.Type)
}

// freePort binds and releases an ephemeral port so the caller can be told which
// one to connect to. The window between release and re-bind is tiny, and the
// Rust client retries, so the race is not observable.
func freePort() (int, error) {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return 0, err
	}
	port := l.Addr().(*net.TCPAddr).Port
	return port, l.Close()
}

func main() {
	port, err := freePort()
	if err != nil {
		fmt.Fprintln(os.Stderr, "cannot allocate a port:", err)
		os.Exit(1)
	}

	srv := cs104.NewServer(handler{})
	if os.Getenv("IECP5_DEBUG") != "" {
		srv.LogMode(true)
	}

	go func() {
		// Give the listener a moment to come up before announcing it.
		time.Sleep(50 * time.Millisecond)
		emit(map[string]interface{}{"event": "ready", "addr": fmt.Sprintf("127.0.0.1:%d", port)})
	}()

	// Publish spontaneous data so the master sees unsolicited traffic too.
	go func() {
		ticker := time.NewTicker(500 * time.Millisecond)
		defer ticker.Stop()
		for range ticker.C {
			_ = asdu.MeasuredValueFloatCP56Time2a(srv,
				asdu.CauseOfTransmission{Cause: asdu.Spontaneous}, 1,
				asdu.MeasuredValueFloatInfo{Ioa: 700, Value: 3.5, Time: time.Now()})
		}
	}()

	if err := srv.ListenAndServer(fmt.Sprintf("127.0.0.1:%d", port)); err != nil {
		fmt.Fprintln(os.Stderr, "server stopped:", err)
		os.Exit(1)
	}
}

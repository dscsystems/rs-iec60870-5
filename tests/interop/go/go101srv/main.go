// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.
//
// go101srv is an IEC 60870-5-101 secondary station built on go-iecp5, carried
// over the TCP encapsulation transport so the test needs no serial hardware.
//
// It binds an ephemeral port, prints it, then reports everything it receives as
// JSON lines on stdout.
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

// srv is captured by the handler: the cs101 server interface has no Connect
// argument, so replies go through the station itself.
var srv *cs101.Server

type handler struct{}

func (handler) InterrogationHandler(pack *asdu.ASDU, qoi asdu.QualifierOfInterrogation) error {
	emit(map[string]interface{}{"event": "interrogation", "qoi": byte(qoi), "ca": uint(pack.CommonAddr)})

	if qoi != asdu.QOIStation {
		pack.Coa.IsNegative = true
		return pack.SendReplyMirror(srv, asdu.ActivationCon)
	}
	if err := pack.SendReplyMirror(srv, asdu.ActivationCon); err != nil {
		return err
	}

	coa := asdu.CauseOfTransmission{Cause: asdu.InterrogatedByStation}
	ca := pack.CommonAddr

	if err := asdu.Single(srv, false, coa, ca,
		asdu.SinglePointInfo{Ioa: 100, Value: true, Qds: asdu.QDSGood},
		asdu.SinglePointInfo{Ioa: 101, Value: false, Qds: asdu.QDSInvalid},
	); err != nil {
		return err
	}
	if err := asdu.Double(srv, false, coa, ca,
		asdu.DoublePointInfo{Ioa: 200, Value: asdu.DPIDeterminedOn},
	); err != nil {
		return err
	}
	if err := asdu.MeasuredValueScaled(srv, false, coa, ca,
		asdu.MeasuredValueScaledInfo{Ioa: 380, Value: -1234},
	); err != nil {
		return err
	}
	if err := asdu.MeasuredValueFloat(srv, false, coa, ca,
		asdu.MeasuredValueFloatInfo{Ioa: 400, Value: 22.5},
	); err != nil {
		return err
	}
	if err := asdu.SingleCP56Time2a(srv,
		asdu.CauseOfTransmission{Cause: asdu.Spontaneous}, ca,
		asdu.SinglePointInfo{Ioa: 600, Value: true,
			Time: time.Date(2026, 8, 17, 12, 34, 56, 789000000, time.UTC)},
	); err != nil {
		return err
	}
	return pack.SendReplyMirror(srv, asdu.ActivationTerm)
}

func (handler) CounterInterrogationHandler(pack *asdu.ASDU, qcc asdu.QualifierCountCall) error {
	emit(map[string]interface{}{"event": "counter_interrogation", "request": byte(qcc.Request)})
	if err := pack.SendReplyMirror(srv, asdu.ActivationCon); err != nil {
		return err
	}
	if err := asdu.IntegratedTotals(srv, false,
		asdu.CauseOfTransmission{Cause: asdu.RequestByGeneralCounter}, pack.CommonAddr,
		asdu.BinaryCounterReadingInfo{Ioa: 501, Value: asdu.BinaryCounterReading{CounterReading: 99}},
	); err != nil {
		return err
	}
	return pack.SendReplyMirror(srv, asdu.ActivationTerm)
}

func (handler) ReadHandler(pack *asdu.ASDU, ioa asdu.InfoObjAddr) error {
	emit(map[string]interface{}{"event": "read", "ioa": uint(ioa)})
	return asdu.MeasuredValueFloat(srv, false,
		asdu.CauseOfTransmission{Cause: asdu.Request}, pack.CommonAddr,
		asdu.MeasuredValueFloatInfo{Ioa: ioa, Value: 7.5})
}

func (handler) ClockSyncHandler(pack *asdu.ASDU, t time.Time) error {
	emit(map[string]interface{}{"event": "clock_sync", "time": t.UTC().Format(time.RFC3339Nano)})
	return pack.SendReplyMirror(srv, asdu.ActivationCon)
}

func (handler) ResetProcessHandler(pack *asdu.ASDU, qrp asdu.QualifierOfResetProcessCmd) error {
	emit(map[string]interface{}{"event": "reset_process", "qrp": byte(qrp)})
	return pack.SendReplyMirror(srv, asdu.ActivationCon)
}

func (handler) DelayAcquisitionHandler(pack *asdu.ASDU, msec uint16) error {
	emit(map[string]interface{}{"event": "delay_acquisition", "msec": msec})
	return pack.SendReplyMirror(srv, asdu.ActivationCon)
}

func (handler) ASDUHandlerAll(pack *asdu.ASDU, _ int) error {
	emit(map[string]interface{}{
		"event": "asdu",
		"type":  strings.Trim(pack.Type.String(), "TID<>"),
		"cause": byte(pack.Coa.Cause),
		"ca":    uint(pack.CommonAddr),
	})
	return nil
}

func (handler) ASDUHandler(pack *asdu.ASDU) error {
	switch pack.Type {
	case asdu.C_SC_NA_1:
		cmd := pack.GetSingleCmd()
		emit(map[string]interface{}{
			"event": "single_cmd", "ioa": uint(cmd.Ioa), "value": cmd.Value,
			"qual": byte(cmd.Qoc.Qual), "select": cmd.Qoc.InSelect,
		})
	case asdu.C_DC_NA_1:
		cmd := pack.GetDoubleCmd()
		emit(map[string]interface{}{"event": "double_cmd", "ioa": uint(cmd.Ioa), "value": byte(cmd.Value)})
	case asdu.C_SE_NC_1:
		cmd := pack.GetSetpointFloatCmd()
		emit(map[string]interface{}{"event": "setpoint_float", "ioa": uint(cmd.Ioa), "value": cmd.Value})
	default:
		return nil
	}
	if err := pack.SendReplyMirror(srv, asdu.ActivationCon); err != nil {
		return err
	}
	return pack.SendReplyMirror(srv, asdu.ActivationTerm)
}

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
	addr := fmt.Sprintf("127.0.0.1:%d", port)

	cfg := cs101.DefaultConfig()
	cfg.Transport = cs101.TransportTCPServer
	cfg.TCP = cs101.TCPConfig{Address: addr}
	cfg.LinkAddress = 1
	cfg.LinkAddrSize = 1
	cfg.TimeoutSendLinkMsg = 20 * time.Millisecond

	srv = cs101.NewServer(handler{})
	srv.SetConfig(cfg)
	srv.SetParams(asdu.ParamsStandard101)
	srv.SetLogMode(os.Getenv("IECP5_DEBUG") != "")

	if err := srv.Start(); err != nil {
		fmt.Fprintln(os.Stderr, "start failed:", err)
		os.Exit(1)
	}
	emit(map[string]interface{}{"event": "ready", "addr": addr})

	// Buffer a periodic value now and then, so the master's class 2 polls have
	// something to collect even when nothing else is happening.
	go func() {
		for range time.Tick(500 * time.Millisecond) {
			_ = asdu.MeasuredValueFloat(srv, false,
				asdu.CauseOfTransmission{Cause: asdu.Periodic}, 1,
				asdu.MeasuredValueFloatInfo{Ioa: 700, Value: 3.5})
		}
	}()

	timeout := 30 * time.Second
	if v := os.Getenv("IECP5_TIMEOUT_SECS"); v != "" {
		var secs int
		if _, err := fmt.Sscanf(v, "%d", &secs); err == nil && secs > 0 {
			timeout = time.Duration(secs) * time.Second
		}
	}
	time.Sleep(timeout)
	_ = srv.Close()
}

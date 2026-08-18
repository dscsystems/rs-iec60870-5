// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.
//
// go103cli is an IEC 60870-5-103 primary station built on go-iecp5, carried
// over the TCP encapsulation transport.
//
// Both this crate and go-iecp5 implement the 103 master only, so interoperation
// is verified by driving the same simulated protection device with each and
// comparing what the device saw and what the master reported.
//
// Usage: go103cli <host:port>
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"sync"
	"time"

	"github.com/riclolsen/go-iecp5/cs103"
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

// deviceAddr is the link and common address of the simulated relay.
const deviceAddr byte = 3

type handler struct{}

// TimeTaggedHandler receives ASDU 1 and 2: events, interrogation replies and
// command acknowledgements.
func (handler) TimeTaggedHandler(a *cs103.ASDU, info cs103.TimeTaggedInfo) error {
	ev := map[string]interface{}{
		"event": "time_tagged",
		"ca":    a.CommonAddr,
		"cause": byte(a.Coa),
		"fun":   info.Fun,
		"inf":   info.Inf,
		"dpi":   byte(info.Dpi),
		"sin":   info.Sin,
	}
	if !info.Time.IsZero() {
		ev["time"] = info.Time.UTC().Format(time.RFC3339Nano)
	}
	emit(ev)
	return nil
}

// MeasurandsHandler receives ASDU 3 and 9.
func (handler) MeasurandsHandler(a *cs103.ASDU, info cs103.MeasurandsInfo) error {
	values := make([]float64, 0, len(info.Values))
	for _, m := range info.Values {
		values = append(values, m.Float64())
	}
	emit(map[string]interface{}{
		"event": "measurands", "ca": a.CommonAddr, "cause": byte(a.Coa),
		"fun": info.Fun, "inf": info.Inf, "values": values,
	})
	return nil
}

// IdentificationHandler receives ASDU 5.
func (handler) IdentificationHandler(a *cs103.ASDU, info cs103.IdentificationInfo) error {
	emit(map[string]interface{}{
		"event": "identification", "ca": a.CommonAddr,
		"col": info.Col, "ascii": info.ASCII,
	})
	return nil
}

// GITerminationHandler receives ASDU 8.
func (handler) GITerminationHandler(a *cs103.ASDU, scn byte) error {
	emit(map[string]interface{}{"event": "gi_termination", "ca": a.CommonAddr, "scn": scn})
	return nil
}

// ASDUHandler receives everything else, including the ASDU 6 time-sync mirror.
func (handler) ASDUHandler(a *cs103.ASDU) error {
	emit(map[string]interface{}{
		"event": "asdu", "ca": a.CommonAddr,
		"type": byte(a.Type), "cause": byte(a.Coa),
	})
	return nil
}

func (handler) ASDUHandlerAll(_ *cs103.ASDU) error { return nil }

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: go103cli <host:port>")
		os.Exit(2)
	}

	cfg := cs103.DefaultConfig()
	cfg.Transport = cs103.TransportTCPClient
	cfg.TCP = cs103.TCPConfig{Address: os.Args[1]}
	cfg.LinkAddress = deviceAddr
	cfg.TimeoutSendLinkMsg = 20 * time.Millisecond
	cfg.AutoInit = true

	opt := cs103.NewOption()
	if err := opt.SetConfig(cfg); err != nil {
		fmt.Fprintln(os.Stderr, "bad config:", err)
		os.Exit(2)
	}

	cli := cs103.NewClient(handler{}, opt)
	if cli == nil {
		fmt.Fprintln(os.Stderr, "the client could not be created")
		os.Exit(1)
	}
	cli.SetLogMode(os.Getenv("IECP5_DEBUG") != "")
	cli.SetOnDeviceActiveHandler(func(_ *cs103.Client, addr byte) {
		emit(map[string]interface{}{"event": "device_active", "addr": addr})
	})

	if err := cli.Start(); err != nil {
		fmt.Fprintln(os.Stderr, "start failed:", err)
		os.Exit(1)
	}

	for i := 0; i < 200 && !cli.IsLinkActive(); i++ {
		time.Sleep(50 * time.Millisecond)
	}
	if !cli.IsLinkActive() {
		emit(map[string]interface{}{"event": "error", "error": "link never became active"})
		os.Exit(1)
	}
	emit(map[string]interface{}{"event": "link_active"})

	// Let the automatic time sync and general interrogation complete first.
	time.Sleep(700 * time.Millisecond)

	if err := cli.GeneralCommand(deviceAddr,
		cs103.FunOvercurrentProtection, cs103.InfAutoRecloserActive,
		cs103.DCOOn, 42); err != nil {
		emit(map[string]interface{}{"event": "error", "op": "command", "error": err.Error()})
	}

	time.Sleep(300 * time.Millisecond)
	if err := cli.GeneralInterrogation(deviceAddr, 7); err != nil {
		emit(map[string]interface{}{"event": "error", "op": "gi", "error": err.Error()})
	}

	time.Sleep(500 * time.Millisecond)
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

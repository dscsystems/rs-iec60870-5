// Independent file-transfer peer for the Rust interoperability suite.
package main

import (
	"encoding/json"
	"fmt"
	"github.com/riclolsen/go-iecp5/asdu"
	"github.com/riclolsen/go-iecp5/cs104"
	"github.com/riclolsen/go-iecp5/filetransfer"
	"os"
	"sync"
	"time"
)

var mu sync.Mutex

func emit(v any) { mu.Lock(); defer mu.Unlock(); _ = json.NewEncoder(os.Stdout).Encode(v) }
func fixture() []byte {
	data := make([]byte, 600)
	for i := range data {
		data[i] = byte(i*17 + 3)
	}
	return data
}

type server struct{ sender *filetransfer.Sender }

func (h server) InterrogationHandler(asdu.Connect, *asdu.ASDU, asdu.QualifierOfInterrogation) error {
	return nil
}
func (h server) CounterInterrogationHandler(asdu.Connect, *asdu.ASDU, asdu.QualifierCountCall) error {
	return nil
}
func (h server) ReadHandler(asdu.Connect, *asdu.ASDU, asdu.InfoObjAddr) error { return nil }
func (h server) ClockSyncHandler(asdu.Connect, *asdu.ASDU, time.Time) error   { return nil }
func (h server) ResetProcessHandler(asdu.Connect, *asdu.ASDU, asdu.QualifierOfResetProcessCmd) error {
	return nil
}
func (h server) DelayAcquisitionHandler(asdu.Connect, *asdu.ASDU, uint16) error { return nil }
func (h server) ASDUHandlerAll(asdu.Connect, *asdu.ASDU, int) error             { return nil }
func (h server) ASDUHandler(c asdu.Connect, a *asdu.ASDU) error {
	_, err := h.sender.Handle(c, a)
	if a.Type == asdu.F_AF_NA_1 && a.GetAckFileOrSection().Afq.Action == asdu.AFQPosAckFile {
		emit(map[string]any{"event": "file_ack"})
	}
	return err
}

type client struct{ receiver *filetransfer.Receiver }

func (h client) InterrogationHandler(asdu.Connect, *asdu.ASDU) error               { return nil }
func (h client) CounterInterrogationHandler(asdu.Connect, *asdu.ASDU) error        { return nil }
func (h client) ReadHandler(asdu.Connect, *asdu.ASDU) error                        { return nil }
func (h client) TestCommandHandler(asdu.Connect, *asdu.ASDU) error                 { return nil }
func (h client) ClockSyncHandler(asdu.Connect, *asdu.ASDU) error                   { return nil }
func (h client) ResetProcessHandler(asdu.Connect, *asdu.ASDU) error                { return nil }
func (h client) DelayAcquisitionHandler(asdu.Connect, *asdu.ASDU) error            { return nil }
func (h client) ASDUHandlerAll(asdu.Connect, *asdu.ASDU, *cs104.Server, int) error { return nil }
func (h client) ASDUHandler(c asdu.Connect, a *asdu.ASDU, _ *cs104.Server, _ int) error {
	_, err := h.receiver.Handle(c, a)
	return err
}
func main() {
	if len(os.Args) != 3 {
		os.Exit(1)
	}
	if os.Args[1] == "server" {
		store := filetransfer.NewMemStore()
		_ = store.Write(100, 2, fixture())
		srv := cs104.NewServer(server{filetransfer.NewSender(store).SetSectionSize(256)})
		go func() { time.Sleep(100 * time.Millisecond); emit(map[string]any{"event": "ready", "addr": os.Args[2]}) }()
		if err := srv.ListenAndServer(os.Args[2]); err != nil {
			panic(err)
		}
	} else {
		r := filetransfer.NewReceiver(nil)
		r.SetFileHandler(func(e filetransfer.Entry, data []byte) {
			values := make([]int, len(data))
			for i, v := range data {
				values[i] = int(v)
			}
			emit(map[string]any{"event": "file", "data": values, "size": len(data)})
		})
		r.SetDirectoryHandler(func(ca asdu.CommonAddr, d []asdu.DirectoryInfo) {
			emit(map[string]any{"event": "directory", "count": len(d)})
		})
		opt := cs104.NewOption()
		_ = opt.AddRemoteServer(os.Args[2])
		c := cs104.NewClient(client{r}, opt)
		c.SetOnConnectHandler(func(c *cs104.Client) { c.SendStartDt() })
		c.SetOnActivatedHandler(func(c *cs104.Client) {
			go func() {
				if err := r.RequestDirectory(c, 1); err != nil {
					panic(err)
				}
				if err := r.RequestFile(c, 1, 100, 2); err != nil {
					panic(err)
				}
			}()
		})
		if err := c.Start(); err != nil {
			panic(err)
		}
		time.Sleep(30 * time.Second)
	}
	fmt.Println("exit")
}

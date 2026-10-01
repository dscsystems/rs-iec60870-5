/* Test peer using only lib60870's public API. */
#include "cs104_connection.h"
#include "cs104_slave.h"
#include "cs101_master.h"
#include "cs101_slave.h"
#include "hal_serial.h"
#include "hal_thread.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdatomic.h>
#include <pthread.h>

static CS101_AppLayerParameters params;
static atomic_bool active;
static unsigned char file_data[600];
static unsigned char received[600];
static int received_size;
static unsigned char checksum(const unsigned char* data, int size) {
    unsigned char sum=0; for (int i=0;i<size;i++) sum+=data[i]; return sum;
}
static void report(CS101_ASDU a) {
    printf("{\"event\":\"asdu\",\"type\":%d,\"cause\":%d,\"oa\":%d,\"ca\":%d,\"payload\":[",CS101_ASDU_getTypeID(a),CS101_ASDU_getCOT(a),CS101_ASDU_getOA(a),CS101_ASDU_getCA(a));
    unsigned char* payload=CS101_ASDU_getPayload(a);
    for(int i=0;i<CS101_ASDU_getPayloadSize(a);i++) printf("%s%d",i?",":"",payload[i]);
    printf("]}\n");
}
static CS101_ASDU message(InformationObject io,int cause) {
    CS101_ASDU a=CS101_ASDU_create(params,false,cause,7,1,false,false);
    if(!CS101_ASDU_addInformationObject(a,io)) exit(3);
    InformationObject_destroy(io); return a;
}
static void server_send(IMasterConnection c, InformationObject io,int cause) {
    CS101_ASDU a=message(io,cause); IMasterConnection_sendASDU(c,a);CS101_ASDU_destroy(a);
}
static void client_send(CS104_Connection c, InformationObject io,int cause) {
    CS101_ASDU a=message(io,cause); if(!CS104_Connection_sendASDU(c,a)) exit(4);CS101_ASDU_destroy(a);
}
/* lib60870 invokes receive handlers with its connection lock held. Queue
   replies here; the main thread sends them after the callback returns. */
static pthread_mutex_t queue_lock=PTHREAD_MUTEX_INITIALIZER;
static CS101_ASDU queued[128];
static int queue_count;
static void queue_send(CS104_Connection c,InformationObject io,int cot) {
    (void)c;CS101_ASDU a=message(io,cot);
    pthread_mutex_lock(&queue_lock);
    if(queue_count==128)exit(11);
    queued[queue_count++]=a;pthread_mutex_unlock(&queue_lock);
}
static void drain(CS104_Connection c) {
    CS101_ASDU batch[128];int n;
    pthread_mutex_lock(&queue_lock);n=queue_count;
    memcpy(batch,queued,n*sizeof(CS101_ASDU));queue_count=0;
    pthread_mutex_unlock(&queue_lock);
    for(int i=0;i<n;i++){if(!CS104_Connection_sendASDU(c,batch[i]))exit(12);CS101_ASDU_destroy(batch[i]);}
}
static bool interrogation(void* p,IMasterConnection c,CS101_ASDU a,uint8_t qoi) {
    report(a); IMasterConnection_sendACT_CON(c,a,qoi!=20);
    if(qoi==20) {
        server_send(c,(InformationObject)SinglePointInformation_create(NULL,100,true,IEC60870_QUALITY_GOOD),20);
        server_send(c,(InformationObject)MeasuredValueShort_create(NULL,400,22.5f,IEC60870_QUALITY_INVALID),20);
        IMasterConnection_sendACT_TERM(c,a);
    }
    return true;
}
static bool server_asdu(void* p,IMasterConnection c,CS101_ASDU a) {
    report(a); int type=CS101_ASDU_getTypeID(a);
    InformationObject io=CS101_ASDU_getElement(a,0);
    if(!io) exit(5);
    if(type==C_SC_NA_1) {
        SingleCommand sc=(SingleCommand)io;
        printf("{\"event\":\"command\",\"ioa\":%d,\"value\":%d,\"select\":%d}\n",InformationObject_getObjectAddress(io),SingleCommand_getState(sc),SingleCommand_isSelect(sc));
        IMasterConnection_sendACT_CON(c,a,false);IMasterConnection_sendACT_TERM(c,a);
    } else if(type==F_SC_NA_1) {
        FileCallOrSelect call=(FileCallOrSelect)io;
        int action=FileCallOrSelect_getSCQ(call)&15;
        if(action==0) {
            struct sCP56Time2a time;CP56Time2a_createFromMsTimestamp(&time,1787056496789ULL);CP56Time2a_setDayOfWeek(&time,2);
            server_send(c,(InformationObject)FileDirectory_create(NULL,100,2,600,32,&time),5);
        } else if(action==1 || action==2) {
            server_send(c,(InformationObject)SectionReady_create(NULL,100,2,1,600,false),13);
        } else if(action==6) {
            int max=FileSegment_GetMaxDataSize(params);
            for(int offset=0;offset<600;offset+=max) {
                int n=600-offset;if(n>max)n=max;
                server_send(c,(InformationObject)FileSegment_create(NULL,100,2,1,file_data+offset,n),13);
            }
            server_send(c,(InformationObject)FileLastSegmentOrSection_create(NULL,100,2,1,3,checksum(file_data,600)),13);
        }
    } else if(type==F_AF_NA_1) {
        int action=FileACK_getAFQ((FileACK)io)&15;
        if(action==3)server_send(c,(InformationObject)FileLastSegmentOrSection_create(NULL,100,2,1,1,0),13);
        if(action==1)printf("{\"event\":\"file_ack\"}\n");
    }
    InformationObject_destroy(io);return true;
}
static void connection(void* p,CS104_Connection c,CS104_ConnectionEvent e) {
    if(e==CS104_CONNECTION_STARTDT_CON_RECEIVED)atomic_store(&active,true);
    if(e==CS104_CONNECTION_STOPDT_CON_RECEIVED)printf("{\"event\":\"stopped\"}\n");
}
static bool client_asdu(void* p,int address,CS101_ASDU a) {
    CS104_Connection c=p;report(a);
    int type=CS101_ASDU_getTypeID(a);
    if(type<120)return true;
    InformationObject io=CS101_ASDU_getElement(a,0);if(!io)exit(6);
    if(type==F_FR_NA_1) {
        queue_send(c,(InformationObject)FileCallOrSelect_create(NULL,100,2,0,1),13);
    } else if(type==F_SR_NA_1) {
        queue_send(c,(InformationObject)FileCallOrSelect_create(NULL,100,2,SectionReady_getNameOfSection((SectionReady)io),6),13);
    } else if(type==F_SG_NA_1) {
        FileSegment seg=(FileSegment)io;
        int n=FileSegment_getLengthOfSegment(seg);
        if(received_size+n>600)exit(7);
        memcpy(received+received_size,FileSegment_getSegmentData(seg),n);received_size+=n;
    } else if(type==F_LS_NA_1) {
        FileLastSegmentOrSection last=(FileLastSegmentOrSection)io;
        int nos=FileLastSegmentOrSection_getNameOfSection(last);
        if(FileLastSegmentOrSection_getLSQ(last)==3) {
            /* The fixture has multiple sections. Validate each against its
               portion of the deterministic source file. */
            int start=nos==1?0:(nos-1)*256;
            int n=received_size-start;
            if(checksum(received+start,n)!=FileLastSegmentOrSection_getCHS(last))exit(8);
            queue_send(c,(InformationObject)FileACK_create(NULL,100,2,nos,3),13);
        } else {
            if(received_size!=600 || memcmp(received,file_data,600))exit(9);
            printf("{\"event\":\"file\",\"size\":%d}\n",received_size);
            queue_send(c,(InformationObject)FileACK_create(NULL,100,2,nos,1),13);
        }
    }
    InformationObject_destroy(io);return true;
}
static void vectors(void) {
    struct sCP56Time2a time;CP56Time2a_createFromMsTimestamp(&time,1787056496789ULL);CP56Time2a_setDayOfWeek(&time,2);
    InformationObject ios[]={
        (InformationObject)FileReady_create(NULL,100,2,600,true),
        (InformationObject)SectionReady_create(NULL,100,2,1,600,false),
        (InformationObject)FileCallOrSelect_create(NULL,100,2,1,6),
        (InformationObject)FileLastSegmentOrSection_create(NULL,100,2,1,3,42),
        (InformationObject)FileACK_create(NULL,100,2,1,3),
        (InformationObject)FileSegment_create(NULL,100,2,1,file_data,236),
        (InformationObject)FileDirectory_create(NULL,100,2,600,32,&time)
    };
    for(int i=0;i<7;i++){CS101_ASDU a=message(ios[i],i==6?5:13);report(a);CS101_ASDU_destroy(a);}
}
static atomic_bool gi_complete;
static bool received101(void* p,int address,CS101_ASDU a) {(void)p;(void)address;report(a);if(CS101_ASDU_getTypeID(a)==100 && CS101_ASDU_getCOT(a)==10)atomic_store(&gi_complete,true);return true;}
static int serial_peer(const char* mode,const char* path) {
    /* PTYs carry FT1.2 bytes but cannot model physical parity errors. */
    SerialPort port=SerialPort_create(path,9600,8,'N',1);
    if(!strcmp(mode,"server101")) {
        CS101_Slave slave=CS101_Slave_create(port,NULL,NULL,IEC60870_LINK_LAYER_UNBALANCED);
        CS101_Slave_setLinkLayerAddress(slave,1);
        CS101_Slave_getLinkLayerParameters(slave)->addressLength=1;
        params=CS101_Slave_getAppLayerParameters(slave);
        params->sizeOfCOT=1;params->sizeOfCA=1;params->sizeOfIOA=2;
        CS101_Slave_setInterrogationHandler(slave,interrogation,NULL);
        CS101_Slave_setASDUHandler(slave,server_asdu,NULL);
        if(!SerialPort_open(port))return 13;
        printf("{\"event\":\"ready\",\"addr\":\"%s\"}\n",path);
        for(;;){CS101_Slave_run(slave);Thread_sleep(1);}
    } else {
        CS101_Master master=CS101_Master_create(port,NULL,NULL,IEC60870_LINK_LAYER_UNBALANCED);
        CS101_Master_getLinkLayerParameters(master)->addressLength=1;
        params=CS101_Master_getAppLayerParameters(master);
        params->sizeOfCOT=1;params->sizeOfCA=1;params->sizeOfIOA=2;
        CS101_Master_setASDUReceivedHandler(master,received101,NULL);
        CS101_Master_addSlave(master,1);
        if(!SerialPort_open(port))return 14;
        bool sent=false, command_sent=false;
        for(;;) {
            if(!sent && CS101_Master_isChannelReady(master,1)) {
                CS101_Master_useSlaveAddress(master,1);
                CS101_Master_sendInterrogationCommand(master,CS101_COT_ACTIVATION,1,20);
                sent=true;
            }
            if(atomic_load(&gi_complete) && !command_sent && CS101_Master_isChannelReady(master,1)) {
                InformationObject sc=(InformationObject)SingleCommand_create(NULL,500,true,true,0);
                CS101_Master_sendProcessCommand(master,CS101_COT_ACTIVATION,1,sc);InformationObject_destroy(sc);command_sent=true;
            }
            CS101_Master_pollSingleSlave(master,1);CS101_Master_run(master);Thread_sleep(10);
        }
    }
}
int main(int argc,char** argv) {
    setvbuf(stdout,NULL,_IOLBF,0);
    for(int i=0;i<600;i++)file_data[i]=(unsigned char)(i*17+3);
    if(argc<2)return 1;
    if(!strcmp(argv[1],"server101") || !strcmp(argv[1],"client101"))return serial_peer(argv[1],argv[2]);
    if(!strcmp(argv[1],"server")) {
        CS104_Slave slave=CS104_Slave_create(100,100);
        CS104_Slave_setLocalAddress(slave,"127.0.0.1");CS104_Slave_setLocalPort(slave,atoi(argv[2]));
        params=CS104_Slave_getAppLayerParameters(slave);
        CS104_Slave_setInterrogationHandler(slave,interrogation,NULL);
        CS104_Slave_setASDUHandler(slave,server_asdu,NULL);
        CS104_Slave_start(slave);if(!CS104_Slave_isRunning(slave))return 2;
        printf("{\"event\":\"ready\",\"addr\":\"127.0.0.1:%s\"}\n",argv[2]);
        for(;;)Thread_sleep(20);
    } else {
        CS104_Connection c=CS104_Connection_create("127.0.0.1",argc>2?atoi(argv[2]):2404);
        params=CS104_Connection_getAppLayerParameters(c);params->originatorAddress=7;
        if(!strcmp(argv[1],"vectors")){vectors();CS104_Connection_destroy(c);return 0;}
        CS104_Connection_setConnectionHandler(c,connection,NULL);
        CS104_Connection_setASDUReceivedHandler(c,client_asdu,c);
        if(!CS104_Connection_connect(c))return 2;
        CS104_Connection_sendStartDT(c);
        for(int n=0;!atomic_load(&active);n++){if(n>500)return 10;Thread_sleep(10);}
        CS104_Connection_sendInterrogationCommand(c,CS101_COT_ACTIVATION,1,20);
        client_send(c,(InformationObject)SingleCommand_create(NULL,500,true,true,0),6);
        client_send(c,(InformationObject)FileCallOrSelect_create(NULL,0,0,0,0),5);
        client_send(c,(InformationObject)FileCallOrSelect_create(NULL,100,2,0,1),13);
        for(;;){drain(c);Thread_sleep(10);}
    }
}

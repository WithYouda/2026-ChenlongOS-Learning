

## 从0开始boot

1、先烧写一下官方的镜像，这个镜像有个boot fat32的分区. 
    默认大小是16Mb，这个不够大，先用分区工具拓展。 

2、编译文件系统，制作镜像。 

3、make sg2002 (要执行所有的patch脚本)
```bash
./rootfs/patch_sstatus.sh && ./rootfs/patch_table_entry.sh  && ./rootfs/patch_uspace.sh 
```

4、把ext4_100m.img和StarryOS_sg2002.bin都拷贝到fat分区。 

5、上板进入uboot执行以下命令
```bash
setenv starry 'fatload mmc 0:1 0x89000000 ext4_100m.img; fatload mmc 0:1 0x80200000 StarryOS_sg2002.bin; go 0x80200000'
run starry
```
BUSY_BOX_DIR=/home/kingdom/StarryOS/busybox-1.37.0/_install

mkdir -p ./tmp/mnt_rootfs
sudo mount -o loop ext4_100m.img ./tmp/mnt_rootfs
sudo cp -a  ${BUSY_BOX_DIR}/* ./tmp/mnt_rootfs/
sudo cp ../src/init.sh ./tmp/mnt_rootfs/init.sh
sudo chmod +x ./tmp/mnt_rootfs/init.sh
sync
sudo umount ./tmp/mnt_rootfs

package com.example.chunkcourier.blockentity;

import net.minecraft.world.level.block.entity.BlockEntity;

public class ParcelBlockEntity extends BlockEntity {
    private int contents;

    public void tick() {
        if (contents > 0) {
            contents--;
            setChanged();
        }
    }
}

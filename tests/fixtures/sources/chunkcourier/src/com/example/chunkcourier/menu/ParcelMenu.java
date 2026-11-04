package com.example.chunkcourier.menu;

import net.minecraft.world.entity.player.PlayerEntity;
import net.minecraft.world.inventory.AbstractContainerMenu;

public class ParcelMenu extends AbstractContainerMenu {
    @Override
    public boolean stillValid(PlayerEntity player) {
        return true;
    }
}
